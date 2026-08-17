# Retained Tart template for FlanForge iOS jobs. The Cirrus Labs Tahoe/Xcode
# image supplies macOS, Xcode, iOS SDKs/simulators, Homebrew, and SSH. This
# template adds only the native-iOS Rust/CI toolchain and Tailscale. Tailscale
# authentication is disabled by default.

packer {
  required_plugins {
    tart = {
      version = ">= 1.12.0"
      source  = "github.com/cirruslabs/tart"
    }
  }
}

variable "base_image" {
  type    = string
  default = "ghcr.io/cirruslabs/macos-tahoe-xcode:latest"
}

variable "vm_name" {
  type    = string
  default = "flanforge-base"
}

variable "cpu_count" {
  type    = number
  default = 8
}

variable "memory_gb" {
  type    = number
  default = 12
}

# Zero preserves the base image's sparse disk size.
variable "disk_size_gb" {
  type    = number
  default = 0
}

# Optional deployment-specific dependency routing. Leaving it empty keeps the
# image portable; jobs must then provide their own Cargo registry definition.
variable "services_root_domain" {
  type    = string
  default = ""
}

variable "guest_ssh_public_key_file" {
  type = string
}

variable "forgejo_runner_binary_file" {
  type = string
}

variable "forgejo_runner_version" {
  type = string
}

variable "prewarm_enabled" {
  type    = bool
  default = true
}

variable "prewarm_target" {
  type    = string
  default = "iPhone 17 Pro"
}

variable "rust_toolchain" {
  type    = string
  default = "stable"
}

variable "just_version" {
  type    = string
  default = "1.57.0"
}

variable "nextest_version" {
  type    = string
  default = "0.9.140"
}

variable "sccache_version" {
  type    = string
  default = "0.17.0"
}

variable "tailscale_enabled" {
  type    = bool
  default = false
}

# The tailnet join key is delivered as a file so it never reaches a guest
# command line or the environment of any other provisioning script.
variable "tailscale_preauth_key_file" {
  type = string
}

variable "tailscale_login_server" {
  type    = string
  default = ""

  validation {
    condition     = var.tailscale_login_server == "" || can(regex("^https?://[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?(:[0-9]{1,5})?/?$", var.tailscale_login_server))
    error_message = "Tailscale login server must be an HTTP(S) origin."
  }
}

variable "tailscale_hostname" {
  type    = string
  default = ""

  validation {
    condition     = var.tailscale_hostname == "" || can(regex("^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?$", var.tailscale_hostname))
    error_message = "Tailscale hostname must be a valid host label."
  }
}

variable "tailscale_extra_args" {
  type    = string
  default = ""

  validation {
    condition     = length(var.tailscale_extra_args) <= 4096
    error_message = "Tailscale extra arguments must not exceed 4096 characters."
  }
}

variable "admin_password_file" {
  type = string
}

# The public base image's documented bootstrap password. It authenticates the
# SecureToken-holding account when that password is replaced.
variable "base_admin_password" {
  type      = string
  default   = "admin"
  sensitive = true

  validation {
    condition     = length(var.base_admin_password) > 0
    error_message = "Base administrator password must not be empty."
  }
}

variable "disable_admin_password_change" {
  type    = bool
  default = false
}

source "tart-cli" "base" {
  vm_base_name = var.base_image
  vm_name      = var.vm_name
  cpu_count    = var.cpu_count
  memory_gb    = var.memory_gb
  disk_size_gb = var.disk_size_gb > 0 ? var.disk_size_gb : null
  headless     = true

  # Cirrus Labs' Xcode images expose this bootstrap account. CI jobs do not:
  # provisioning creates a separate, non-admin `runner` account below.
  ssh_username = "admin"
  ssh_password = var.base_admin_password
  ssh_timeout  = "300s"
}

locals {
  provisioning_environment = [
    "RUST_TOOLCHAIN=${var.rust_toolchain}",
    "JUST_VERSION=${var.just_version}",
    "NEXTEST_VERSION=${var.nextest_version}",
    "SCCACHE_VERSION=${var.sccache_version}",
    "SERVICES_ROOT_DOMAIN=${var.services_root_domain}",
    "FORGEJO_RUNNER_VERSION=${var.forgejo_runner_version}",
    "PREWARM_ENABLED=${var.prewarm_enabled}",
    "PREWARM_TARGET=${var.prewarm_target}",
    "TAILSCALE_ENABLED=${var.tailscale_enabled}",
  ]
}

build {
  sources = ["source.tart-cli.base"]

  provisioner "file" {
    source      = var.guest_ssh_public_key_file
    destination = "/tmp/flanforge-guest.pub"
  }

  provisioner "file" {
    source      = var.forgejo_runner_binary_file
    destination = "/tmp/flanforge-forgejo-runner"
  }

  provisioner "shell" {
    scripts = [
      "scripts/provision-runner-user.sh",
      "scripts/provision-ios-toolchain.sh",
      "scripts/provision-network.sh",
    ]
    environment_vars = local.provisioning_environment
  }

  provisioner "file" {
    source      = var.tailscale_preauth_key_file
    destination = "/tmp/flanforge-tailscale-preauth-key"
  }

  provisioner "shell" {
    script = "scripts/provision-tailscale.sh"
    environment_vars = [
      "TAILSCALE_ENABLED=${var.tailscale_enabled}",
      "TAILSCALE_LOGIN_SERVER=${var.tailscale_login_server}",
      "TAILSCALE_HOSTNAME=${var.tailscale_hostname}",
      "TAILSCALE_EXTRA_ARGS=${var.tailscale_extra_args}",
    ]
  }

  provisioner "shell" {
    scripts = [
      "scripts/provision-custom.sh",
      "scripts/provision-simulator.sh",
      "scripts/provision-verify.sh",
    ]
    environment_vars = local.provisioning_environment
  }

  # Change the public base image's known administrator password last. The
  # secret is uploaded as a file so it never enters a Packer command line.
  provisioner "file" {
    source      = var.admin_password_file
    destination = "/tmp/flanforge-admin-password"
  }

  provisioner "shell" {
    script = "scripts/provision-admin-password.sh"
    environment_vars = [
      "DISABLE_ADMIN_PASSWORD_CHANGE=${var.disable_admin_password_change}",
      "BASE_ADMIN_PASSWORD=${var.base_admin_password}",
    ]
  }
}

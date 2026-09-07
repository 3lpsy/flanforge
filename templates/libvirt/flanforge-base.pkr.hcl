packer {
  required_version = ">= 1.11.0, < 2.0.0"

  required_plugins {
    qemu = {
      version = "= 1.1.5"
      source  = "github.com/hashicorp/qemu"
    }
  }
}

variable "source_image_url" {
  type = string

  validation {
    condition     = can(regex("^https://[A-Za-z0-9]", var.source_image_url))
    error_message = "The source image must use HTTPS."
  }
}

variable "source_image_sha256" {
  type = string

  validation {
    condition     = can(regex("^[a-f0-9]{64}$", var.source_image_sha256))
    error_message = "The source image SHA-256 must be 64 lowercase hex characters."
  }
}

variable "output_directory" {
  type = string
}

variable "image_name" {
  type = string

  validation {
    condition     = can(regex("^[A-Za-z0-9][A-Za-z0-9._-]{0,63}\\.qcow2$", var.image_name))
    error_message = "The image name must be a bounded qcow2 file name."
  }
}

variable "cpu_count" {
  type = number

  validation {
    condition     = var.cpu_count >= 1 && var.cpu_count <= 64
    error_message = "CPU count must be between 1 and 64."
  }
}

variable "memory_mb" {
  type = number

  validation {
    condition     = var.memory_mb >= 2048 && var.memory_mb <= 131072
    error_message = "Memory must be between 2048 and 131072 MiB."
  }
}

variable "disk_size_mb" {
  type = number

  validation {
    condition     = var.disk_size_mb >= 16384 && var.disk_size_mb <= 1048576
    error_message = "Disk size must be between 16384 and 1048576 MiB."
  }
}

variable "build_ssh_private_key_file" {
  type = string
}

variable "build_ssh_public_key_file" {
  type = string
}

variable "forgejo_runner_binary_file" {
  type = string
}

variable "forgejo_runner_version" {
  type = string

  validation {
    condition     = can(regex("^[0-9]+\\.[0-9]+\\.[0-9]+$", var.forgejo_runner_version))
    error_message = "Forgejo Runner version must be semantic."
  }
}

variable "forgejo_runner_sha256" {
  type = string

  validation {
    condition     = can(regex("^[a-f0-9]{64}$", var.forgejo_runner_sha256))
    error_message = "Forgejo Runner SHA-256 must be lowercase hex."
  }
}

variable "cargo_index_url" {
  type = string

  validation {
    condition     = var.cargo_index_url == "" || can(regex("^https://[A-Za-z0-9]", var.cargo_index_url))
    error_message = "The Cargo index must use HTTPS."
  }
}

variable "dependency_proxy_url" {
  type = string

  validation {
    condition     = var.dependency_proxy_url == "" || can(regex("^https://[A-Za-z0-9]", var.dependency_proxy_url))
    error_message = "The dependency proxy must use HTTPS."
  }
}

variable "npm_registry_url" {
  type = string

  validation {
    condition     = var.npm_registry_url == "" || can(regex("^https://[A-Za-z0-9]", var.npm_registry_url))
    error_message = "The npm registry must use HTTPS."
  }
}

variable "python_index_url" {
  type = string

  validation {
    condition     = var.python_index_url == "" || can(regex("^https://[A-Za-z0-9]", var.python_index_url))
    error_message = "The Python index must use HTTPS."
  }
}

variable "pytorch_index_url" {
  type = string

  validation {
    condition     = var.pytorch_index_url == "" || can(regex("^https://[A-Za-z0-9]", var.pytorch_index_url))
    error_message = "The PyTorch index must use HTTPS."
  }
}

variable "maven_repository_url" {
  type = string

  validation {
    condition     = var.maven_repository_url == "" || can(regex("^https://[A-Za-z0-9]", var.maven_repository_url))
    error_message = "The Maven repository must use HTTPS."
  }
}

variable "google_maven_repository_url" {
  type = string

  validation {
    condition     = var.google_maven_repository_url == "" || can(regex("^https://[A-Za-z0-9]", var.google_maven_repository_url))
    error_message = "The Google Maven repository must use HTTPS."
  }
}

variable "container_registry_mirror" {
  type = string
}

variable "dependency_ca_file" {
  type = string
}

variable "gradle_init_script_file" {
  type = string
}

variable "guest_manifest_file" {
  type = string
}

variable "privileged_account_enabled" {
  type    = bool
  default = true
}

# Like the tailnet key, the optional sudo password travels only as a file.
variable "privileged_sudo_password_file" {
  type = string
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
    condition     = var.tailscale_login_server == "" || can(regex("^https://[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?(:[0-9]{1,5})?/?$", var.tailscale_login_server))
    error_message = "The Tailscale login server must be an HTTPS origin."
  }
}

variable "tailscale_hostname" {
  type    = string
  default = ""

  validation {
    condition     = var.tailscale_hostname == "" || can(regex("^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?$", var.tailscale_hostname))
    error_message = "The Tailscale hostname must be a valid host label."
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

locals {
  guest_environment = [
    "CARGO_INDEX_URL=${var.cargo_index_url}",
    "CHILLED_PROXY_URL=${var.dependency_proxy_url}",
    "CONTAINER_REGISTRY_MIRROR=${var.container_registry_mirror}",
    "FORGEJO_RUNNER_SHA256=${var.forgejo_runner_sha256}",
    "FORGEJO_RUNNER_VERSION=${var.forgejo_runner_version}",
    "GOOGLE_MAVEN_REPOSITORY_URL=${var.google_maven_repository_url}",
    "MAVEN_REPOSITORY_URL=${var.maven_repository_url}",
    "NPM_REGISTRY_URL=${var.npm_registry_url}",
    "PRIVILEGED_ACCOUNT_ENABLED=${var.privileged_account_enabled}",
    "PYTHON_INDEX_URL=${var.python_index_url}",
    "PYTORCH_INDEX_URL=${var.pytorch_index_url}",
    "TAILSCALE_ENABLED=${var.tailscale_enabled}",
  ]

  # Kept out of every other script's environment; the key travels as a file.
  tailscale_environment = [
    "TAILSCALE_ENABLED=${var.tailscale_enabled}",
    "TAILSCALE_EXTRA_ARGS=${var.tailscale_extra_args}",
    "TAILSCALE_HOSTNAME=${var.tailscale_hostname}",
    "TAILSCALE_LOGIN_SERVER=${var.tailscale_login_server}",
  ]
}

source "qemu" "base" {
  accelerator          = "kvm"
  boot_wait            = "5s"
  cd_label             = "cidata"
  cpus                 = var.cpu_count
  disk_compression     = true
  disk_image           = true
  disk_interface       = "virtio"
  disk_size            = "${var.disk_size_mb}M"
  format               = "qcow2"
  headless             = true
  iso_checksum         = "sha256:${var.source_image_sha256}"
  iso_url              = var.source_image_url
  memory               = var.memory_mb
  net_device           = "virtio-net"
  output_directory     = var.output_directory
  shutdown_command     = "sudo /usr/local/libexec/flanforge-image-finalize"
  skip_compaction      = false
  ssh_username         = "packer"
  ssh_timeout          = "15m"
  ssh_private_key_file = var.build_ssh_private_key_file
  vm_name              = var.image_name

  cd_content = {
    "meta-data" = templatefile("${path.root}/cloud-init/build-meta-data.pkrtpl.hcl", {})
    "user-data" = templatefile("${path.root}/cloud-init/build-user-data.pkrtpl.hcl", {
      ssh_public_key = trimspace(file(var.build_ssh_public_key_file))
    })
  }
}

build {
  sources = ["source.qemu.base"]

  provisioner "file" {
    source      = var.forgejo_runner_binary_file
    destination = "/tmp/flanforge-forgejo-runner"
  }

  provisioner "file" {
    source      = var.dependency_ca_file
    destination = "/tmp/flanforge-dependency-ca.pem"
  }

  provisioner "file" {
    source      = var.gradle_init_script_file
    destination = "/tmp/flanforge-chilled-proxy.init.gradle"
  }

  provisioner "file" {
    source      = "${path.root}/../shared/dependencies/checksums.sh"
    destination = "/tmp/flanforge-dependency-checksums.sh"
  }

  provisioner "file" {
    source      = "${path.root}/scripts/subordinate-ids.sh"
    destination = "/tmp/flanforge-subordinate-ids.sh"
  }

  provisioner "file" {
    source      = "${path.root}/systemd/flanforge-tailscale-operator.service"
    destination = "/tmp/flanforge-tailscale-operator.service"
  }

  provisioner "shell" {
    scripts = [
      "${path.root}/scripts/provision-selinux.sh",
      "${path.root}/scripts/provision-base.sh",
      "${path.root}/scripts/provision-runner.sh",
    ]
    environment_vars = local.guest_environment
    timeout          = "30m"
  }

  # Uploaded immediately before its only consumer, which deletes the file.
  provisioner "file" {
    source      = var.privileged_sudo_password_file
    destination = "/tmp/flanforge-privileged-sudo-password"
  }

  provisioner "shell" {
    scripts          = ["${path.root}/scripts/provision-privileged.sh"]
    environment_vars = local.guest_environment
    timeout          = "10m"
  }

  # Uploaded immediately before their only consumer, which deletes the key.
  provisioner "file" {
    source      = var.tailscale_preauth_key_file
    destination = "/tmp/flanforge-tailscale-preauth-key"
  }

  provisioner "file" {
    source      = "${path.root}/../shared/tailscale/arguments.sh"
    destination = "/tmp/flanforge-tailscale-arguments.sh"
  }

  provisioner "shell" {
    scripts          = ["${path.root}/scripts/provision-tailscale.sh"]
    environment_vars = local.tailscale_environment
    timeout          = "15m"
  }

  provisioner "file" {
    source      = "${path.root}/guest/flanforge-guest-ready"
    destination = "/tmp/flanforge-guest-ready"
  }

  provisioner "file" {
    source      = "${path.root}/guest/flanforge-guest-recycle"
    destination = "/tmp/flanforge-guest-recycle"
  }

  provisioner "shell" {
    scripts = [
      "${path.root}/scripts/provision-containers.sh",
      "${path.root}/scripts/provision-dependencies.sh",
      "${path.root}/scripts/provision-guest-agent.sh",
      "${path.root}/scripts/provision-verify.sh",
    ]
    environment_vars = local.guest_environment
    timeout          = "30m"
  }

  provisioner "file" {
    direction   = "download"
    source      = "/tmp/flanforge-guest-manifest.json"
    destination = var.guest_manifest_file
  }

  provisioner "file" {
    source      = "${path.root}/scripts/finalize.sh"
    destination = "/tmp/flanforge-image-finalize"
  }

  provisioner "shell" {
    inline = [
      "sudo install -D -m 0700 -o root -g root /tmp/flanforge-image-finalize /usr/local/libexec/flanforge-image-finalize",
      "rm -f /tmp/flanforge-image-finalize",
    ]
  }
}

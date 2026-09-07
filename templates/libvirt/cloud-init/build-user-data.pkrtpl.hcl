#cloud-config
${yamlencode({
  disable_root = true
  ssh_pwauth   = false
  users = [
    "default",
    {
      name                = "packer"
      gecos               = "FlanForge image builder"
      groups              = ["wheel"]
      lock_passwd         = true
      shell               = "/bin/bash"
      ssh_authorized_keys = [ssh_public_key]
      sudo                = ["ALL=(ALL) NOPASSWD:ALL"]
    },
  ]
})}

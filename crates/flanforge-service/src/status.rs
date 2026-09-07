use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServicePlatform {
    Launchd,
    Systemd,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceStatus {
    pub platform: ServicePlatform,
    pub label: &'static str,
    pub binary: PathBuf,
    pub definition: PathBuf,
    pub is_installed: bool,
    pub state: String,
    pub pid: Option<u32>,
    pub last_exit: Option<i32>,
    pub user: Option<String>,
}

impl ServiceStatus {
    pub fn print(&self) {
        println!("label:      {}", self.label);
        println!("binary:     {}", describe(&self.binary));
        println!("definition: {}", describe(&self.definition));
        println!("state:      {}", self.state);
        if let Some(user) = &self.user {
            println!("user:       {user}");
        }
        if let Some(pid) = self.pid {
            println!("pid:        {pid}");
        }
        if let Some(code) = self.last_exit {
            println!("last exit:  {code}");
        }
    }
}

fn describe(path: &std::path::Path) -> String {
    match std::fs::metadata(path) {
        Ok(metadata) => format!("{} ({} bytes)", path.display(), metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            format!("{} (absent)", path.display())
        }
        Err(_) => format!("{} (unavailable)", path.display()),
    }
}

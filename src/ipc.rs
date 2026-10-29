use anyhow::{Context, Result, bail};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{OpenOptionsExt, PermissionsExt},
            net::UnixDatagram,
        },
    },
    path::PathBuf,
};

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Toggle,
    Show,
    Hide,
    Passthrough,
    Clear,
    Undo,
    Redo,
    Quit,
}

impl Command {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "toggle" => Self::Toggle,
            "show" => Self::Show,
            "hide" => Self::Hide,
            "passthrough" => Self::Passthrough,
            "clear" => Self::Clear,
            "undo" => Self::Undo,
            "redo" => Self::Redo,
            "quit" => Self::Quit,
            _ => return None,
        })
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::Show => "show",
            Self::Hide => "hide",
            Self::Passthrough => "passthrough",
            Self::Clear => "clear",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::Quit => "quit",
        }
    }
}

fn socket_path() -> Result<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .context("XDG_RUNTIME_DIR is not set; run inside your Wayland session")?;
    let display = std::env::var("WAYLAND_DISPLAY").context("WAYLAND_DISPLAY is not set")?;
    let display: String = display
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(PathBuf::from(runtime).join(format!("livepaint-{display}.sock")))
}

pub fn send(command: Command) -> Result<bool> {
    let socket = UnixDatagram::unbound()?;
    match socket.send_to(command.as_str().as_bytes(), socket_path()?) {
        Ok(_) => Ok(true),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            Ok(false)
        }
        Err(e) => Err(e).context("Cannot contact LivePaint"),
    }
}

pub struct Guard {
    path: PathBuf,
    _lock: File,
}

impl Guard {
    pub fn bind() -> Result<(Self, UnixDatagram)> {
        let path = socket_path()?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path.with_extension("lock"))?;
        // SAFETY: flock operates on this valid file descriptor. Keeping File alive holds the lock.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("Another LivePaint instance is starting; try again in a moment");
        }
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let socket = UnixDatagram::bind(&path).context("Cannot create control socket")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        socket.set_nonblocking(true)?;
        Ok((Self { path, _lock: lock }, socket))
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

//! Single instance: a second `veta` launch hands its files to the running
//! window instead of opening another one.
//!
//! The first instance listens on a Unix domain socket in a per-user private
//! directory; later launches connect, send their file paths and exit. File
//! permissions on the socket decide who may connect (on Windows, the ACL of
//! the user's local app data folder). See #60 and decision 32.
//!
//! Protocol: the client writes `veta 1\n`, then each absolute path followed
//! by a NUL byte, and shuts down its write side; the server answers `ok`.

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
use uds_windows::{UnixListener, UnixStream};

const HEADER: &[u8] = b"veta 1\n";
const ACK: &[u8] = b"ok";
/// Largest message accepted, so a stray client can't exhaust memory.
const MAX_MESSAGE: u64 = 1 << 20;
const TIMEOUT: Duration = Duration::from_secs(3);
const SOCKET_NAME: &str = "veta.sock";

/// Result of [`claim`].
#[derive(Debug)]
pub enum Claim {
    /// Another instance took the files; this process should exit.
    Forwarded,
    /// This is the first instance and now listens for others.
    First(UnixListener),
    /// Single instance isn't available (no directory, permissions,
    /// unresponsive instance): run on our own.
    Alone,
}

/// The private directory holding the socket: `$XDG_RUNTIME_DIR/veta` on
/// Linux (falling back to the config dir), `%LOCALAPPDATA%\veta` on Windows,
/// the config dir on macOS. `$VETA_INSTANCE_DIR` overrides it.
pub fn default_dir() -> Option<PathBuf> {
    let env = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = env("VETA_INSTANCE_DIR") {
        return Some(dir);
    }
    if cfg!(windows) {
        return env("LOCALAPPDATA").map(|d| d.join("veta"));
    }
    if cfg!(target_os = "linux")
        && let Some(run) = env("XDG_RUNTIME_DIR")
    {
        return Some(run.join("veta"));
    }
    crate::config::config_dir()
}

/// Hands `files` to a running instance listening in `dir`, or becomes the
/// first instance there.
pub fn claim(dir: &Path, files: &[PathBuf]) -> Claim {
    if prepare_dir(dir).is_err() {
        return Claim::Alone;
    }
    let socket = dir.join(SOCKET_NAME);
    // Twice: if another launch binds between our connect and bind, the
    // second round reaches it.
    for _ in 0..2 {
        match UnixStream::connect(&socket) {
            Ok(stream) => {
                return match send(stream, files) {
                    Ok(()) => Claim::Forwarded,
                    // Someone listens but doesn't answer: don't take over.
                    Err(_) => Claim::Alone,
                };
            }
            Err(_) => {
                // Nobody listens: a leftover from a crash, or no file.
                let _ = std::fs::remove_file(&socket);
                if let Ok(listener) = UnixListener::bind(&socket) {
                    return Claim::First(listener);
                }
            }
        }
    }
    Claim::Alone
}

/// Creates `dir` readable by the current user only.
fn prepare_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Fails unless we own the directory.
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn send(mut stream: UnixStream, files: &[PathBuf]) -> io::Result<()> {
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let mut message = HEADER.to_vec();
    for file in files {
        let path = std::path::absolute(file).unwrap_or_else(|_| file.clone());
        if let Some(bytes) = path_bytes(&path) {
            message.extend_from_slice(&bytes);
            message.push(0);
        }
    }
    stream.write_all(&message)?;
    stream.shutdown(Shutdown::Write)?;
    let mut answer = Vec::new();
    stream.take(ACK.len() as u64).read_to_end(&mut answer)?;
    if answer == ACK {
        Ok(())
    } else {
        Err(io::Error::other("no answer from the running instance"))
    }
}

/// Reads one client's message and answers it. Returns the paths sent.
pub fn receive(mut stream: UnixStream) -> io::Result<Vec<PathBuf>> {
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let mut message = Vec::new();
    (&mut stream)
        .take(MAX_MESSAGE + 1)
        .read_to_end(&mut message)?;
    if message.len() as u64 > MAX_MESSAGE {
        return Err(io::Error::other("message too large"));
    }
    let Some(body) = message.strip_prefix(HEADER) else {
        return Err(io::Error::other("not a Veta message"));
    };
    let paths = body
        .split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .filter_map(path_from_bytes)
        .collect();
    stream.write_all(ACK)?;
    Ok(paths)
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Option<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    Some(path.as_os_str().as_bytes().to_vec())
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Option<Vec<u8>> {
    path.to_str().map(|s| s.as_bytes().to_vec())
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    std::str::from_utf8(bytes).ok().map(PathBuf::from)
}

/// The listener of the first instance, picked up by [`incoming`].
static LISTENER: Mutex<Option<UnixListener>> = Mutex::new(None);

/// Makes `listener` the source of [`incoming`].
pub fn listen(listener: UnixListener) {
    if let Ok(mut slot) = LISTENER.lock() {
        *slot = Some(listener);
    }
}

/// File lists sent by later launches. Empty if this process doesn't listen.
pub fn incoming() -> impl iced::futures::Stream<Item = Vec<PathBuf>> {
    iced::stream::channel(16, async |mut output| {
        let listener = LISTENER.lock().ok().and_then(|mut slot| slot.take());
        if let Some(listener) = listener {
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    if let Ok(paths) = receive(stream) {
                        let _ = output.try_send(paths);
                    }
                }
            });
        }
        std::future::pending::<()>().await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use veta_testkit::TempDir;

    #[test]
    fn second_launch_forwards_its_files() {
        let dir = TempDir::new();
        let Claim::First(listener) = claim(dir.path(), &[]) else {
            panic!("first launch should listen");
        };
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            receive(stream).unwrap()
        });
        let files = vec![PathBuf::from("/data/a b.parquet"), PathBuf::from("c.csv")];
        assert!(matches!(claim(dir.path(), &files), Claim::Forwarded));
        let received = server.join().unwrap();
        assert_eq!(received[0], PathBuf::from("/data/a b.parquet"));
        assert!(
            received[1].is_absolute(),
            "relative paths are made absolute"
        );
        assert!(received[1].ends_with("c.csv"));
    }

    #[test]
    fn stale_socket_is_replaced() {
        let dir = TempDir::new();
        let first = claim(dir.path(), &[]);
        assert!(matches!(first, Claim::First(_)));
        drop(first); // like a crash: the socket file stays
        assert!(dir.path().join(SOCKET_NAME).exists());
        assert!(matches!(claim(dir.path(), &[]), Claim::First(_)));
    }

    #[cfg(unix)]
    #[test]
    fn directory_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let sub = dir.join("veta");
        let _claim = claim(&sub, &[]);
        let mode = std::fs::metadata(&sub).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn foreign_messages_are_rejected() {
        let dir = TempDir::new();
        let Claim::First(listener) = claim(dir.path(), &[]) else {
            panic!("first launch should listen");
        };
        let socket = dir.path().join(SOCKET_NAME);
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            receive(stream)
        });
        let mut client = UnixStream::connect(&socket).unwrap();
        client.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        assert!(server.join().unwrap().is_err());
    }
}

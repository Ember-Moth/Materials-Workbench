//! The running application's end of the channel (ai-integration design
//! §5.1): it listens at its [`Endpoint`], checks each client's token, and
//! serves MCP on the connection with a [`Server`] whose calls go to the
//! application's main thread through a [`Dispatcher`].

use std::io;
use std::sync::Arc;
use std::time::Duration;

use rmcp::ServiceExt as _;
use tokio::io::{
    AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _,
};
use tokio::sync::oneshot;

use crate::endpoint::{self, Endpoint, HANDSHAKE, Hello, MAX_HANDSHAKE, Welcome};
use crate::{Access, Dispatcher, Server};

/// How long a client has to say hello.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// The application listening at an endpoint, on a thread of its own.
/// Dropping it stops listening, closes every connection and removes the
/// socket and the token file; once it returns, the endpoint is free for
/// the next start (on Windows the pipe closes with the thread).
#[derive(Debug)]
pub struct Listening {
    endpoint: Endpoint,
    token: String,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Listening {
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        // Quick: the thread drops its connections without waiting for them.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // Only the files of this start: another instance may have taken over.
        let file = self.endpoint.token_file();
        if endpoint::read_token(&file).ok().flatten().as_deref() == Some(self.token.as_str()) {
            let _ = std::fs::remove_file(&file);
            #[cfg(unix)]
            let _ = std::fs::remove_file(self.endpoint.socket());
        }
    }
}

/// Starts listening at `endpoint` with a new token; connections' calls go
/// to `dispatcher`. Fails when another instance of the application listens
/// there already, or the directory is not the user's own.
pub fn listen(endpoint: &Endpoint, dispatcher: Dispatcher) -> io::Result<Listening> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let token = endpoint::new_token();
    // Bound here, so that failures are the caller's to report; served on
    // the thread.
    let bound = {
        let _entered = runtime.enter();
        platform::bind(endpoint, &token)
    };
    let listener = match bound {
        Ok(listener) => listener,
        Err(e) => {
            // The caller may be on a runtime of its own, where dropping one
            // would block.
            runtime.shutdown_background();
            return Err(e);
        }
    };
    let (stop, stopped) = oneshot::channel();
    let shared: Arc<str> = token.as_str().into();
    let thread = std::thread::Builder::new()
        .name("mcp-listener".into())
        .spawn(move || {
            runtime.block_on(async move {
                tokio::select! {
                    _ = stopped => {}
                    result = platform::accept(listener, shared, dispatcher) => {
                        if let Err(e) = result {
                            log::error!("stopped listening for MCP clients: {e}");
                        }
                    }
                }
            });
            // Connections end with the runtime; nothing waits for them.
            runtime.shutdown_background();
        })?;
    Ok(Listening {
        endpoint: endpoint.clone(),
        token,
        stop: Some(stop),
        thread: Some(thread),
    })
}

/// Reads one line of the handshake, refusing long ones.
pub(crate) async fn read_line<R: AsyncRead + Unpin>(
    reader: &mut tokio::io::BufReader<R>,
) -> io::Result<String> {
    let mut line = String::new();
    let read = (&mut *reader)
        .take(MAX_HANDSHAKE)
        .read_line(&mut line)
        .await?;
    if read == 0 || !line.ends_with('\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the handshake ended early",
        ));
    }
    Ok(line)
}

pub(crate) async fn write_line<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &impl serde::Serialize,
) -> io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await
}

/// What the client may open: the directories it allows, checked again here.
fn access(hello: &Hello) -> Result<Access, String> {
    let access = Access::new(&hello.allow).map_err(|e| e.to_string())?;
    Ok(match &hello.cwd {
        Some(cwd) if cwd.is_absolute() => access.with_base(cwd.clone()),
        _ => access,
    })
}

/// Serves one connection: the handshake, then MCP until either end closes.
async fn serve<S>(stream: S, token: Arc<str>, dispatcher: Dispatcher)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut read = tokio::io::BufReader::new(read);
    let hello = match tokio::time::timeout(HELLO_TIMEOUT, read_line(&mut read)).await {
        Ok(Ok(line)) => serde_json::from_str::<Hello>(&line).map_err(|e| e.to_string()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("no handshake in time".into()),
    };
    let accepted = hello.and_then(|hello| {
        if hello.handshake != HANDSHAKE {
            return Err(format!(
                "handshake version {} is not {HANDSHAKE}: the application and \
                 materials-workbench mcp are different versions",
                hello.handshake
            ));
        }
        if !endpoint::same_token(&hello.token, &token) {
            return Err("wrong token".into());
        }
        access(&hello)
    });
    let access = match accepted {
        Ok(access) => access,
        Err(error) => {
            log::warn!("refused an MCP client: {error}");
            let refusal = Welcome {
                ok: false,
                error: Some(error),
            };
            let _ = write_line(&mut write, &refusal).await;
            return;
        }
    };
    let welcome = Welcome {
        ok: true,
        error: None,
    };
    if write_line(&mut write, &welcome).await.is_err() {
        return;
    }
    let server = Server::dispatching(access, dispatcher);
    match server.serve((read, write)).await {
        Ok(service) => {
            if let Err(e) = service.waiting().await {
                log::debug!("an MCP session ended with an error: {e}");
            }
        }
        Err(e) => log::debug!("an MCP client did not start a session: {e}"),
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;
    use tokio::net::UnixListener;

    use crate::endpoint::{current_uid, make_private_dir};

    /// The longest socket path the system takes: `sun_path` holds it with
    /// its terminating NUL.
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    const MAX_SOCKET_PATH: usize = 103;
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd")))]
    const MAX_SOCKET_PATH: usize = 107;

    pub fn bind(endpoint: &Endpoint, token: &str) -> io::Result<UnixListener> {
        make_private_dir(endpoint.dir())?;
        let socket = endpoint.socket();
        if socket.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{} is too long for a Unix domain socket, which takes at most \
                     {MAX_SOCKET_PATH} bytes",
                    socket.display()
                ),
            ));
        }
        if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!(
                    "another instance of the application listens at {}",
                    socket.display()
                ),
            ));
        }
        // Left by an instance that did not quit cleanly.
        match std::fs::remove_file(&socket) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        // The token first: a client that reaches the socket finds it.
        endpoint::write_token(&endpoint.token_file(), token)?;
        let bound = UnixListener::bind(&socket).and_then(|listener| {
            std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
            Ok(listener)
        });
        if bound.is_err() {
            let _ = std::fs::remove_file(endpoint.token_file());
        }
        bound
    }

    pub async fn accept(
        listener: UnixListener,
        token: Arc<str>,
        dispatcher: Dispatcher,
    ) -> io::Result<()> {
        loop {
            let (stream, _) = listener.accept().await?;
            // The directory is the user's alone; this checks again.
            match stream.peer_cred() {
                Ok(peer) if peer.uid() == current_uid() => {}
                _ => {
                    log::warn!("refused an MCP client of another user");
                    continue;
                }
            }
            tokio::spawn(serve(stream, token.clone(), dispatcher.clone()));
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::io::AsRawHandle as _;
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
    use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;

    use crate::endpoint::runs_as_current_user;

    fn instance(endpoint_pipe: &str, first: bool) -> io::Result<NamedPipeServer> {
        ServerOptions::new()
            // Fails when another process holds the name, so that no one can
            // pose as the application.
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create(endpoint_pipe)
    }

    pub struct Pipe {
        name: String,
        next: NamedPipeServer,
    }

    pub fn bind(endpoint: &Endpoint, token: &str) -> io::Result<Pipe> {
        std::fs::create_dir_all(endpoint.dir())?;
        let name = endpoint.pipe();
        let next = instance(&name, true).map_err(|e| {
            // A first instance of a name that exists is refused with
            // ERROR_ACCESS_DENIED; reported as the socket's address in use.
            let kind = match e.kind() {
                io::ErrorKind::PermissionDenied => io::ErrorKind::AddrInUse,
                kind => kind,
            };
            io::Error::new(
                kind,
                format!(
                    "{name} is taken, by another instance of the application or another \
                     program: {e}"
                ),
            )
        })?;
        endpoint::write_token(&endpoint.token_file(), token)?;
        Ok(Pipe { name, next })
    }

    pub async fn accept(pipe: Pipe, token: Arc<str>, dispatcher: Dispatcher) -> io::Result<()> {
        let Pipe { name, mut next } = pipe;
        loop {
            next.connect().await?;
            let connected = std::mem::replace(&mut next, instance(&name, false)?);
            // The token file is the user's alone; this checks again.
            if !client_runs_as_current_user(&connected) {
                log::warn!("refused an MCP client of another user");
                continue;
            }
            tokio::spawn(serve(connected, token.clone(), dispatcher.clone()));
        }
    }

    fn client_runs_as_current_user(pipe: &NamedPipeServer) -> bool {
        let mut process_id = 0;
        // SAFETY: the handle is the open server end of a connected pipe,
        // and `process_id` is valid for writes.
        let found = unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle(), &mut process_id) };
        found != 0 && runs_as_current_user(process_id).unwrap_or(false)
    }
}

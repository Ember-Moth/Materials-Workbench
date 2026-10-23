//! `materials-workbench mcp`'s end of the channel (ai-integration design
//! §5.1): when the application listens, the command connects, says hello
//! with the token, and then copies bytes between stdio and the connection
//! as they come. It does not read MCP: the application's server does.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader};

use crate::Access;
use crate::endpoint::{self, Endpoint, HANDSHAKE, Hello, Welcome};
use crate::listen::{read_line, write_line};

/// How long the application has to answer the hello.
const WELCOME_TIMEOUT: Duration = Duration::from_secs(10);

/// A connection to the application after the handshake.
pub(crate) struct Link {
    read: Box<dyn AsyncBufRead + Send + Unpin>,
    write: Box<dyn AsyncWrite + Send + Unpin>,
}

/// Connects to the application listening at `endpoint`, for a client that
/// may open what `access` allows. `None` when it is not listening: no token
/// file, or one left by an instance that is gone.
pub(crate) async fn connect(endpoint: &Endpoint, access: &Access) -> io::Result<Option<Link>> {
    let Some(stream) = platform::connect(endpoint)? else {
        return Ok(None);
    };
    // Read after connecting: an application that just started has written
    // it by the time its socket takes connections.
    let Some(token) = endpoint::read_token(&endpoint.token_file())? else {
        return Ok(None);
    };
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let hello = Hello {
        handshake: HANDSHAKE,
        token,
        allow: access.allowed().to_vec(),
        cwd: std::env::current_dir().ok(),
    };
    write_line(&mut write, &hello).await?;
    let line = tokio::time::timeout(WELCOME_TIMEOUT, read_line(&mut read))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "the application did not answer"))??;
    let welcome: Welcome =
        serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if !welcome.ok {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            format!(
                "the application refused the connection: {}",
                welcome.error.as_deref().unwrap_or("no reason given")
            ),
        ));
    }
    Ok(Some(Link {
        read: Box::new(read),
        write: Box::new(write),
    }))
}

/// Copies `input` to the application and its answers to `output`, byte for
/// byte, until the application closes the connection. When `input` ends
/// (the client is done), the application learns of it and finishes.
pub(crate) async fn relay<I, O>(link: Link, mut input: I, mut output: O) -> io::Result<()>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
{
    let Link {
        mut read,
        mut write,
    } = link;
    let upstream = async {
        tokio::io::copy(&mut input, &mut write).await?;
        write.shutdown().await
    };
    let downstream = async {
        tokio::io::copy_buf(&mut read, &mut output).await?;
        output.flush().await
    };
    tokio::pin!(upstream, downstream);
    tokio::select! {
        result = &mut downstream => result,
        result = &mut upstream => {
            result?;
            downstream.await
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use tokio::net::UnixStream;

    use crate::endpoint::{check_private_dir, current_uid};

    pub fn connect(endpoint: &Endpoint) -> io::Result<Option<UnixStream>> {
        match check_private_dir(endpoint.dir()) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        }
        let stream = match std::os::unix::net::UnixStream::connect(endpoint.socket()) {
            Ok(stream) => stream,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        stream.set_nonblocking(true)?;
        let stream = UnixStream::from_std(stream)?;
        if stream.peer_cred()?.uid() != current_uid() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the socket belongs to another user",
            ));
        }
        Ok(Some(stream))
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

    /// Windows' "all pipe instances are busy".
    const ERROR_PIPE_BUSY: i32 = 231;

    pub fn connect(endpoint: &Endpoint) -> io::Result<Option<NamedPipeClient>> {
        let pipe = endpoint.pipe();
        // Another client may be taking the waiting instance; the
        // application makes the next one right away.
        for _ in 0..20 {
            match ClientOptions::new().open(&pipe) {
                Ok(client) => return Ok(Some(client)),
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "the application's pipe stayed busy",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::ServiceExt as _;
    use rmcp::model::CallToolRequestParams;
    use serde_json::{Value, json};
    use tokio::io::AsyncReadExt as _;

    use super::*;
    use crate::{Headless, dispatcher, listen};

    fn access(dir: &Path) -> Access {
        Access::new(&[dir.to_path_buf()]).unwrap()
    }

    /// Runs the calls servers hand over on a host of their own thread, as
    /// the application's main thread does.
    fn host_thread(mut calls: crate::Calls, projects: &Path) -> std::thread::JoinHandle<()> {
        let mut host = Headless::new(access(projects));
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            runtime.block_on(async {
                while let Some(call) = calls.next().await {
                    call.run(&mut host);
                }
            });
        })
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn listening_leaves_private_files_and_cleans_up() {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::in_dir(temp.path().join("run"));
        let (dispatcher, _calls) = dispatcher();
        let listening = listen(&endpoint, dispatcher.clone()).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(endpoint.dir()), 0o700);
        assert_eq!(mode(&endpoint.socket()), 0o600);
        assert_eq!(mode(&endpoint.token_file()), 0o600);
        let token = endpoint::read_token(&endpoint.token_file())
            .unwrap()
            .unwrap();
        assert_eq!(token.len(), 64);

        // A second instance does not take over.
        let error = listen(&endpoint, dispatcher.clone()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
        let unchanged = endpoint::read_token(&endpoint.token_file()).unwrap();
        assert_eq!(unchanged.as_deref(), Some(token.as_str()));

        drop(listening);
        assert!(!endpoint.socket().exists());
        assert!(!endpoint.token_file().exists());
        // The next start has a new token.
        let again = listen(&endpoint, dispatcher.clone()).unwrap();
        let next = endpoint::read_token(&endpoint.token_file()).unwrap();
        assert_ne!(next.as_deref(), Some(token.as_str()));
        drop(again);

        // A socket path the system cannot take leaves nothing behind.
        let long = temp.path().join("x".repeat(120));
        let endpoint = Endpoint::in_dir(&long);
        let error = listen(&endpoint, dispatcher).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("too long"), "{error}");
        assert!(!endpoint.token_file().exists());
    }

    #[tokio::test]
    async fn nothing_to_connect_to() {
        let temp = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::in_dir(temp.path().join("run"));
        assert!(
            connect(&endpoint, &Access::default())
                .await
                .unwrap()
                .is_none()
        );
        // Left behind by an application that did not quit cleanly.
        #[cfg(unix)]
        {
            endpoint::make_private_dir(endpoint.dir()).unwrap();
            drop(std::os::unix::net::UnixListener::bind(endpoint.socket()).unwrap());
            endpoint::write_token(&endpoint.token_file(), &endpoint::new_token()).unwrap();
            assert!(endpoint.socket().exists());
            assert!(
                connect(&endpoint, &Access::default())
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_directory_others_can_enter_is_not_trusted() {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::in_dir(temp.path().join("run"));
        let (dispatcher, _calls) = dispatcher();
        let _listening = listen(&endpoint, dispatcher).unwrap();
        let open = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(endpoint.dir(), open).unwrap();
        let error = connect(&endpoint, &Access::default()).await.err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn a_wrong_token_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::in_dir(temp.path().join("run"));
        let (dispatcher, _calls) = dispatcher();
        let _listening = listen(&endpoint, dispatcher).unwrap();
        endpoint::write_token(&endpoint.token_file(), &endpoint::new_token()).unwrap();
        let error = connect(&endpoint, &Access::default()).await.err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused);
        assert!(error.to_string().contains("wrong token"), "{error}");
    }

    /// An application that answers the handshake and then echoes whatever
    /// comes, to see that the relay passes bytes through untouched.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_relay_copies_bytes_as_they_come() {
        let temp = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::in_dir(temp.path().join("run"));
        endpoint::make_private_dir(endpoint.dir()).unwrap();
        let token = endpoint::new_token();
        endpoint::write_token(&endpoint.token_file(), &token).unwrap();
        let listener = tokio::net::UnixListener::bind(endpoint.socket()).unwrap();
        let allowed = temp.path().canonicalize().unwrap();
        let echo = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = tokio::io::split(stream);
            let mut read = BufReader::new(read);
            let hello: Hello = serde_json::from_str(&read_line(&mut read).await.unwrap()).unwrap();
            let welcome = Welcome {
                ok: hello.token == token,
                error: None,
            };
            write_line(&mut write, &welcome).await.unwrap();
            tokio::io::copy_buf(&mut read, &mut write).await.unwrap();
            write.shutdown().await.unwrap();
            hello
        });

        let link = connect(&endpoint, &access(&allowed))
            .await
            .unwrap()
            .unwrap();
        // Not all of it JSON, not all of it text, the last line unfinished.
        let mut sent = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n".to_vec();
        sent.extend_from_slice(b"not json\n\xff\xfe\x00 binary\n");
        sent.extend((0..200_000u32).map(|i| (i % 251) as u8));
        let (mut client, relay_input) = tokio::io::duplex(1024);
        let (relay_output, mut received) = tokio::io::duplex(1024);
        let relayed = tokio::spawn(relay(link, relay_input, relay_output));
        let writer = {
            let sent = sent.clone();
            tokio::spawn(async move {
                client.write_all(&sent).await.unwrap();
                client.shutdown().await.unwrap();
            })
        };
        let mut back = Vec::new();
        received.read_to_end(&mut back).await.unwrap();
        writer.await.unwrap();
        relayed.await.unwrap().unwrap();
        assert_eq!(back.len(), sent.len());
        assert!(back == sent, "the relay changed the bytes");

        let hello = echo.await.unwrap();
        assert_eq!(hello.handshake, HANDSHAKE);
        assert_eq!(hello.allow, vec![allowed]);
        assert_eq!(hello.cwd, std::env::current_dir().ok());
    }

    /// The application's end: each connection gets a server whose calls
    /// run on the host's thread; the tools are those with a window.
    #[tokio::test]
    async fn clients_reach_the_host_through_the_channel() {
        let temp = tempfile::tempdir().unwrap();
        let projects = temp.path().join("projects");
        std::fs::create_dir(&projects).unwrap();
        mw_project_file::samples::install(&projects).unwrap();
        let endpoint = Endpoint::in_dir(temp.path().join("run"));
        let (dispatcher, calls) = dispatcher();
        let listening = listen(&endpoint, dispatcher).unwrap();
        let host = host_thread(calls, &projects);

        let link = connect(&endpoint, &access(&projects))
            .await
            .unwrap()
            .unwrap();
        let client = ().serve((link.read, link.write)).await.unwrap();
        let info = client.peer_info().unwrap();
        let instructions = info.instructions.as_deref().unwrap();
        assert!(instructions.contains("connected to the application"));
        let tools = client.list_all_tools().await.unwrap();
        assert!(tools.iter().any(|t| t.name == "project_open"));
        assert!(tools.iter().all(|t| t.name != "file_open"));

        let path = projects.join("tio2.mwproj");
        let params = CallToolRequestParams::new("project_open")
            .with_arguments(json!({ "path": path }).as_object().unwrap().clone());
        let result = client.call_tool(params).await.unwrap();
        assert_ne!(result.is_error, Some(true), "{result:#?}");
        let output: Value = result.structured_content.unwrap();
        assert_eq!(output["name"], "TiO₂ 相研究");
        client.cancel().await.unwrap();

        // Stopping closes the connections; the host's thread ends with the
        // last dispatcher.
        drop(listening);
        host.join().unwrap();
    }
}

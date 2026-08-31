//! Cross-platform IPC transport abstraction.
//!
//! - Windows: Named pipes (`tokio::net::windows::named_pipe`)
//! - Unix:    Unix domain sockets (`tokio::net::UnixStream` / `UnixListener`)

use std::io::{IoSlice, Result};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

// ── Platform-specific type aliases ───────────────────────────────────

#[cfg(target_os = "windows")]
pub type ServerStream = tokio::net::windows::named_pipe::NamedPipeServer;
#[cfg(target_os = "windows")]
pub type ClientStream = tokio::net::windows::named_pipe::NamedPipeClient;

#[cfg(not(target_os = "windows"))]
pub type ServerStream = tokio::net::UnixStream;
#[cfg(not(target_os = "windows"))]
pub type ClientStream = tokio::net::UnixStream;

// ── Create server / connect client ──────────────────────────────────

/// Create a server-side endpoint and wait for a client connection.
///
/// On Windows this creates a `NamedPipeServer` (connection is implicit when client opens).
/// On Unix this binds a `UnixListener`, accepts one connection, and returns the `UnixStream`.
pub async fn create_server(path: &str) -> std::io::Result<ServerStream> {
    #[cfg(target_os = "windows")]
    {
        tokio::net::windows::named_pipe::ServerOptions::new()
            .first_pipe_instance(true)
            .create(path)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let listener = tokio::net::UnixListener::bind(path)?;
        let (stream, _addr) = listener.accept().await?;
        Ok(stream)
    }
}

/// Connect as a client to an existing server endpoint.
pub async fn connect_client(path: &str) -> std::io::Result<ClientStream> {
    #[cfg(target_os = "windows")]
    {
        tokio::net::windows::named_pipe::ClientOptions::new().open(path)
    }
    #[cfg(not(target_os = "windows"))]
    {
        tokio::net::UnixStream::connect(path).await
    }
}

// ── IpcStream enum (Windows only — Unix uses UnixStream directly) ──

/// Wraps either a server-side or client-side connected stream.
///
/// On Unix both sides are `UnixStream` (identical type).
/// On Windows the server side is `NamedPipeServer` and client side is `NamedPipeClient`.
#[cfg(target_os = "windows")]
pub enum IpcStream {
    Server(ServerStream),
    Client(ClientStream),
}

#[cfg(target_os = "windows")]
impl IpcStream {
    pub fn from_server(s: ServerStream) -> Self {
        IpcStream::Server(s)
    }
    pub fn from_client(s: ClientStream) -> Self {
        IpcStream::Client(s)
    }
}

// ── AsyncRead + AsyncWrite impls (platform-split) ───────────────────

#[cfg(target_os = "windows")]
mod impls {
    use super::*;

    impl AsyncRead for IpcStream {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            match self.get_mut() {
                IpcStream::Server(h) => Pin::new(h).poll_read(cx, buf),
                IpcStream::Client(h) => Pin::new(h).poll_read(cx, buf),
            }
        }
    }

    impl AsyncWrite for IpcStream {
        fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize>> {
            match self.get_mut() {
                IpcStream::Server(h) => Pin::new(h).poll_write(cx, buf),
                IpcStream::Client(h) => Pin::new(h).poll_write(cx, buf),
            }
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
            match self.get_mut() {
                IpcStream::Server(h) => Pin::new(h).poll_flush(cx),
                IpcStream::Client(h) => Pin::new(h).poll_flush(cx),
            }
        }

        fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
            match self.get_mut() {
                IpcStream::Server(h) => Pin::new(h).poll_shutdown(cx),
                IpcStream::Client(h) => Pin::new(h).poll_shutdown(cx),
            }
        }

        fn poll_write_vectored(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bufs: &[IoSlice<'_>],
        ) -> Poll<Result<usize>> {
            match self.get_mut() {
                IpcStream::Server(h) => Pin::new(h).poll_write_vectored(cx, bufs),
                IpcStream::Client(h) => Pin::new(h).poll_write_vectored(cx, bufs),
            }
        }

        fn is_write_vectored(&self) -> bool {
            match self {
                IpcStream::Server(h) => h.is_write_vectored(),
                IpcStream::Client(h) => h.is_write_vectored(),
            }
        }
    }
}

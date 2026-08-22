//! Activation, grants, and lifetime (spec/002 §4).
//!
//! This is the COM analogue at the heart of the broker. A *grant* is one client's refcounted
//! claim on one server. The server starts when the first grant is taken and stops when the last
//! one is released — explicitly, when the holding connection dies, or when the idle timer fires.
//!
//! Two shapes of server, because MCP forces the distinction:
//!
//! * `stdio` launches get **one child process per grant**. An MCP stdio connection is a single
//!   session; sharing one child between two clients would interleave their traffic.
//! * `executable` launches serve a declared endpoint, so one child is **shared and refcounted**
//!   across grants — this is where the idle timeout actually earns its keep.

use crate::audit::AuditLog;
use crate::catalog::Entry;
use crate::consent::{ConsentState, ConsentStore};
use mcp_locator_proto::card::LaunchType;
use serde::Serialize;
use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

static NEXT_GRANT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// How long a server gets to exit on its own after its stdin is closed, before it is killed.
const GRACEFUL_SHUTDOWN: Duration = Duration::from_secs(5);

pub trait DuplexStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> DuplexStream for T {}

#[derive(Debug, Serialize)]
pub struct ActivateResult {
    #[serde(rename = "grantId")]
    pub grant_id: String,
    pub connection: ConnectionInfo,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionInfo {
    #[serde(rename = "type")]
    pub transport: String,
    pub address: String,
}

#[derive(Debug)]
pub enum ActivateError {
    UnknownServer(String),
    ConsentRequired { name: String, state: ConsentState },
    Orphaned(String),
    NotActivatable(String),
    Io(std::io::Error),
}

impl std::fmt::Display for ActivateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownServer(name) => write!(f, "unknown server: {name}"),
            Self::ConsentRequired { name, state } => write!(
                f,
                "consent required for {name} (currently {state:?}); grant it interactively before activating"
            ),
            Self::Orphaned(name) => write!(f, "{name} is orphaned: its launch command is missing"),
            Self::NotActivatable(name) => {
                write!(f, "{name} declares no launch command, so it cannot be started")
            }
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

/// One client's claim on one server.
struct Grant {
    name: String,
    connection: u64,
    client_pid: Option<u32>,
    kind: GrantKind,
}

enum GrantKind {
    /// A child owned solely by this grant, plus the relay task wiring it to the client.
    /// Boxed because it dwarfs the endpoint variant, which carries no payload at all.
    Stdio(Box<StdioGrant>),
    /// A share of the refcounted child behind a declared endpoint.
    Endpoint,
}

struct StdioGrant {
    child: Child,
    relay: JoinHandle<()>,
}

/// A refcounted `executable`-mode server, shared across grants.
struct SharedServer {
    child: Option<Child>,
    refcount: usize,
    idle_timer: Option<JoinHandle<()>>,
}

pub struct Engine {
    pub consent: ConsentStore,
    pub audit: AuditLog,
    grants: HashMap<String, Grant>,
    shared: HashMap<String, SharedServer>,
    #[cfg(windows)]
    job: platform::Job,
}

impl Engine {
    pub fn new(consent: ConsentStore, audit: AuditLog) -> std::io::Result<Self> {
        Ok(Self {
            consent,
            audit,
            grants: HashMap::new(),
            shared: HashMap::new(),
            // Every child joins this job, so children die with the broker even if it is killed
            // rather than shut down. Without it, a force-killed broker orphans MCP servers.
            #[cfg(windows)]
            job: platform::Job::new()?,
        })
    }

    /// Take a grant, starting the server if it is not already running.
    pub async fn activate(
        &mut self,
        entry: &Entry,
        connection: u64,
        client_pid: Option<u32>,
    ) -> Result<ActivateResult, ActivateError> {
        if entry.orphaned {
            return Err(ActivateError::Orphaned(entry.name.clone()));
        }

        // Consent is checked against this card's launch hash, so a card edited since approval
        // reads as stale and is refused here rather than quietly launched.
        let consent = self.consent.evaluate(&entry.name, &entry.launch_hash);
        if consent.state != ConsentState::Granted {
            self.audit.record(
                "activate-refused",
                &entry.name,
                client_pid,
                &format!("{:?}", consent.state),
            );
            return Err(ActivateError::ConsentRequired {
                name: entry.name.clone(),
                state: consent.state,
            });
        }

        let launch = entry
            .card
            .local
            .as_ref()
            .and_then(|l| l.launch.as_ref())
            .ok_or_else(|| ActivateError::NotActivatable(entry.name.clone()))?;

        // Grant ids name relay pipes, so they must not repeat across broker instances either:
        // a restarted broker reusing `g1` would collide with a lingering pipe from the old one.
        let grant_id = format!(
            "g{}-{}",
            std::process::id(),
            NEXT_GRANT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );

        let (kind, connection_info) = match launch.launch_type {
            LaunchType::Stdio => self.start_stdio(entry, &grant_id).await?,
            LaunchType::Executable => self.attach_shared(entry).await?,
        };

        self.grants.insert(
            grant_id.clone(),
            Grant {
                name: entry.name.clone(),
                connection,
                client_pid,
                kind,
            },
        );
        self.audit
            .record("activate", &entry.name, client_pid, &grant_id);

        Ok(ActivateResult {
            grant_id,
            connection: connection_info,
        })
    }

    /// Spawn a child dedicated to one grant and relay it to a per-grant pipe.
    async fn start_stdio(
        &mut self,
        entry: &Entry,
        grant_id: &str,
    ) -> Result<(GrantKind, ConnectionInfo), ActivateError> {
        let listener = platform::RelayListener::create(grant_id).map_err(ActivateError::Io)?;
        let address = listener.address();

        let mut child = self.spawn(entry, true)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ActivateError::Io(std::io::Error::other("child stdin was not piped")))?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ActivateError::Io(std::io::Error::other("child stdout was not piped"))
        })?;

        // The client connects after it receives the address, so accepting has to happen in the
        // background — returning the address is what unblocks the client.
        let relay = tokio::spawn(async move {
            match listener.accept().await {
                Ok(stream) => relay(stream, stdin, stdout).await,
                Err(e) => eprintln!("relay accept failed: {e}"),
            }
        });

        Ok((
            GrantKind::Stdio(Box::new(StdioGrant { child, relay })),
            ConnectionInfo {
                transport: platform::RELAY_TRANSPORT.to_string(),
                address,
            },
        ))
    }

    /// Join (or start) the shared child behind a declared endpoint.
    async fn attach_shared(
        &mut self,
        entry: &Entry,
    ) -> Result<(GrantKind, ConnectionInfo), ActivateError> {
        let endpoint = entry
            .card
            .local
            .as_ref()
            .and_then(|l| l.endpoint.as_ref())
            .ok_or_else(|| ActivateError::NotActivatable(entry.name.clone()))?;

        let server = self
            .shared
            .entry(entry.name.clone())
            .or_insert(SharedServer {
                child: None,
                refcount: 0,
                idle_timer: None,
            });

        // A grant arriving during the idle window cancels the pending shutdown — the COM
        // refcount going back above zero.
        if let Some(timer) = server.idle_timer.take() {
            timer.abort();
        }

        if server.child.is_none() {
            let child = spawn_command(entry, false)?;
            #[cfg(windows)]
            self.job.assign(&child);
            self.shared.get_mut(&entry.name).unwrap().child = Some(child);
        }

        self.shared.get_mut(&entry.name).unwrap().refcount += 1;

        Ok((
            GrantKind::Endpoint,
            ConnectionInfo {
                transport: format!("{:?}", endpoint.endpoint_type).to_lowercase(),
                address: endpoint.address.clone(),
            },
        ))
    }

    fn spawn(&mut self, entry: &Entry, piped: bool) -> Result<Child, ActivateError> {
        let child = spawn_command(entry, piped)?;
        #[cfg(windows)]
        self.job.assign(&child);
        Ok(child)
    }

    /// Release one grant. The server stops when this was the last claim on it.
    pub async fn release(&mut self, grant_id: &str) -> bool {
        let Some(grant) = self.grants.remove(grant_id) else {
            return false;
        };
        self.audit
            .record("release", &grant.name, grant.client_pid, grant_id);
        self.retire(grant).await;
        true
    }

    /// Release every grant held by a connection.
    ///
    /// Connection close subsumes client-process death: a client that crashes, exits, or closes
    /// cleanly all look the same from here, so no process-handle watching is needed to reclaim
    /// its servers.
    pub async fn release_connection(&mut self, connection: u64) {
        let ids: Vec<String> = self
            .grants
            .iter()
            .filter(|(_, g)| g.connection == connection)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.release(&id).await;
        }
    }

    /// Stop a server outright. Without `force` this refuses while other clients hold grants.
    pub async fn deactivate(&mut self, name: &str, force: bool) -> Result<usize, Vec<String>> {
        let holders: Vec<String> = self
            .grants
            .values()
            .filter(|g| g.name == name)
            .map(|g| match g.client_pid {
                Some(pid) => format!("pid {pid}"),
                None => "unknown client".to_string(),
            })
            .collect();

        if !holders.is_empty() && !force {
            return Err(holders);
        }

        let ids: Vec<String> = self
            .grants
            .iter()
            .filter(|(_, g)| g.name == name)
            .map(|(id, _)| id.clone())
            .collect();
        let count = ids.len();
        for id in ids {
            self.release(&id).await;
        }

        // An endpoint-mode server can still be running with no grants during its idle window.
        if let Some(server) = self.shared.get_mut(name) {
            if let Some(timer) = server.idle_timer.take() {
                timer.abort();
            }
            if let Some(child) = server.child.take() {
                shutdown(child).await;
            }
            server.refcount = 0;
        }
        self.audit
            .record("deactivate", name, None, &format!("force={force}"));
        Ok(count)
    }

    async fn retire(&mut self, grant: Grant) {
        match grant.kind {
            GrantKind::Stdio(stdio) => {
                // 1:1 with the grant, so there is nothing left to serve — stop it now.
                stdio.relay.abort();
                shutdown(stdio.child).await;
            }
            GrantKind::Endpoint => {
                let idle = self.idle_timeout(&grant.name);
                let Some(server) = self.shared.get_mut(&grant.name) else {
                    return;
                };
                server.refcount = server.refcount.saturating_sub(1);
                if server.refcount > 0 {
                    return;
                }
                // Last claim released: start the idle timer rather than stopping immediately, so
                // a client reconnecting within the window reuses the running server.
                server.idle_timer =
                    Some(tokio::spawn(async move { tokio::time::sleep(idle).await }));
            }
        }
    }

    /// Idle windows that have elapsed with no new grant: stop those servers. Called on a tick by
    /// the serve loop, keeping the timing decision here rather than in the transport.
    pub async fn reap_idle(&mut self) {
        let expired: Vec<String> = self
            .shared
            .iter()
            .filter(|(_, s)| {
                s.refcount == 0 && s.idle_timer.as_ref().is_some_and(|t| t.is_finished())
            })
            .map(|(name, _)| name.clone())
            .collect();

        for name in expired {
            if let Some(server) = self.shared.get_mut(&name) {
                server.idle_timer = None;
                if let Some(child) = server.child.take() {
                    shutdown(child).await;
                    self.audit.record("idle-shutdown", &name, None, "");
                }
            }
        }
    }

    fn idle_timeout(&self, _name: &str) -> Duration {
        DEFAULT_IDLE_TIMEOUT
    }

    pub fn grant_count(&self, name: &str) -> usize {
        self.grants.values().filter(|g| g.name == name).count()
    }

    /// Whether a server currently has a live process, independent of grant count — an
    /// endpoint-mode server inside its idle window is running with zero grants.
    pub fn is_running(&mut self, name: &str) -> bool {
        if self.grants.values().any(|g| g.name == name) {
            return true;
        }
        self.shared.get_mut(name).is_some_and(|s| s.child.is_some())
    }

    pub fn holders(&self, name: &str) -> Vec<Option<u32>> {
        self.grants
            .values()
            .filter(|g| g.name == name)
            .map(|g| g.client_pid)
            .collect()
    }
}

fn spawn_command(entry: &Entry, piped: bool) -> Result<Child, ActivateError> {
    let launch = entry
        .card
        .local
        .as_ref()
        .and_then(|l| l.launch.as_ref())
        .ok_or_else(|| ActivateError::NotActivatable(entry.name.clone()))?;

    let mut command = Command::new(&launch.command);
    if let Some(args) = &launch.args {
        command.args(args);
    }
    if let Some(cwd) = &launch.cwd {
        command.current_dir(cwd);
    }
    if let Some(env) = entry
        .expanded
        .get("local")
        .and_then(|l| l.get("launch"))
        .and_then(|l| l.get("env"))
        .and_then(|e| e.as_object())
    {
        for (key, value) in env {
            if let Some(value) = value.as_str() {
                command.env(key, value);
            }
        }
    }

    command
        .stdin(if piped { Stdio::piped() } else { Stdio::null() })
        .stdout(if piped { Stdio::piped() } else { Stdio::null() })
        .stderr(Stdio::null())
        // Backstop for the ordinary case; the job object covers a force-killed broker.
        .kill_on_drop(true);

    command.spawn().map_err(ActivateError::Io)
}

/// Close stdin and give the server a moment to exit on its own before killing it. A server
/// killed mid-write can leave the state it was managing torn.
async fn shutdown(mut child: Child) {
    drop(child.stdin.take());
    match tokio::time::timeout(GRACEFUL_SHUTDOWN, child.wait()).await {
        Ok(_) => {}
        Err(_) => {
            let _ = child.kill().await;
        }
    }
}

/// Pump bytes between the client's relay connection and the child's stdio until either side
/// closes. Deliberately dumb: the broker does not parse or rewrite MCP traffic.
async fn relay(
    stream: Box<dyn DuplexStream>,
    mut child_stdin: tokio::process::ChildStdin,
    mut child_stdout: tokio::process::ChildStdout,
) {
    let (mut client_read, mut client_write) = tokio::io::split(stream);
    let to_child = async { tokio::io::copy(&mut client_read, &mut child_stdin).await };
    let to_client = async { tokio::io::copy(&mut child_stdout, &mut client_write).await };

    tokio::select! {
        result = to_child => { if let Err(e) = result { eprintln!("relay (client->server) ended: {e}"); } }
        result = to_client => { if let Err(e) = result { eprintln!("relay (server->client) ended: {e}"); } }
    }
}

#[cfg(windows)]
mod platform {
    use super::DuplexStream;
    use std::os::windows::io::AsRawHandle;
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub const RELAY_TRANSPORT: &str = "pipe";

    /// A job object holding every child the broker starts, configured to kill them when the job
    /// handle closes — which happens when the broker process dies, however it dies.
    pub struct Job(HANDLE);

    // The handle is owned solely by the Engine, which is behind a mutex.
    unsafe impl Send for Job {}

    impl Job {
        pub fn new() -> std::io::Result<Self> {
            // SAFETY: null name and attributes create an unnamed job owned by this process.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `info` matches the class being set and outlives the call.
            let ok = unsafe {
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const std::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if ok == 0 {
                let e = std::io::Error::last_os_error();
                unsafe { CloseHandle(handle) };
                return Err(e);
            }
            Ok(Self(handle))
        }

        pub fn assign(&self, child: &tokio::process::Child) {
            let Some(handle) = child.raw_handle() else {
                return;
            };
            // SAFETY: both handles are live; failure only costs the kill-on-close guarantee.
            unsafe { AssignProcessToJobObject(self.0, handle as HANDLE) };
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    pub struct RelayListener {
        server: NamedPipeServer,
        address: String,
    }

    impl RelayListener {
        pub fn create(grant_id: &str) -> std::io::Result<Self> {
            let address = format!(r"\\.\pipe\mcp-locator\conn\{grant_id}");
            let server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&address)?;
            Ok(Self { server, address })
        }

        pub fn address(&self) -> String {
            self.address.clone()
        }

        pub async fn accept(self) -> std::io::Result<Box<dyn DuplexStream>> {
            self.server.connect().await?;
            Ok(Box::new(self.server))
        }
    }

    /// PID of the process on the other end of a pipe connection. Taken from the connection
    /// itself rather than self-reported, so a client cannot claim someone else's identity.
    pub fn peer_pid(server: &NamedPipeServer) -> Option<u32> {
        use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
        let mut pid = 0u32;
        // SAFETY: the handle is live for the duration of the call.
        let ok = unsafe { GetNamedPipeClientProcessId(server.as_raw_handle() as HANDLE, &mut pid) };
        (ok != 0).then_some(pid)
    }
}

#[cfg(unix)]
mod platform {
    use super::DuplexStream;
    use std::path::PathBuf;
    use tokio::net::{UnixListener, UnixStream};

    pub const RELAY_TRANSPORT: &str = "unix-socket";

    pub struct RelayListener {
        listener: UnixListener,
        path: PathBuf,
    }

    impl RelayListener {
        pub fn create(grant_id: &str) -> std::io::Result<Self> {
            let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
            let path = PathBuf::from(dir).join(format!("mcp-locator-conn-{grant_id}.sock"));
            let _ = std::fs::remove_file(&path);
            let listener = UnixListener::bind(&path)?;
            Ok(Self { listener, path })
        }

        pub fn address(&self) -> String {
            self.path.to_string_lossy().to_string()
        }

        pub async fn accept(self) -> std::io::Result<Box<dyn DuplexStream>> {
            let (stream, _) = self.listener.accept().await?;
            // The socket file has served its purpose once the peer is connected.
            let _ = std::fs::remove_file(&self.path);
            Ok(Box::new(stream))
        }
    }

    /// PID of the peer, taken from the socket rather than self-reported.
    pub fn peer_pid(stream: &UnixStream) -> Option<u32> {
        stream
            .peer_cred()
            .ok()
            .and_then(|cred| cred.pid())
            .map(|pid| pid as u32)
    }
}

pub use platform::peer_pid;

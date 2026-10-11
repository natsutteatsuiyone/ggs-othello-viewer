use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

pub enum NetEvent {
    Status(String),
    LoggedIn,
    Message(Vec<String>),
    /// `retry` is false when the server turned the login down, so trying again cannot help.
    Closed {
        reason: String,
        retry: bool,
    },
}

enum Outgoing {
    Line(String),
    Close,
}

#[derive(Clone)]
pub struct Link {
    outgoing: mpsc::Sender<Outgoing>,
    control: Arc<ConnectionControl>,
}

#[derive(Default)]
struct ConnectionControl {
    /// User cancellation must not trigger automatic reconnection.
    cancelled: AtomicBool,
    /// Either worker has finished, so queued writes must not replace its result.
    closing: AtomicBool,
    stream: Mutex<Option<Arc<TcpStream>>>,
}

impl ConnectionControl {
    fn attach(&self, stream: Arc<TcpStream>) -> bool {
        let mut socket = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        if self.cancelled.load(Ordering::Acquire) {
            let _ = stream.shutdown(Shutdown::Both);
            return false;
        }
        *socket = Some(stream);
        true
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.closing.store(true, Ordering::Release);
        if let Some(stream) = self.stream.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

impl Link {
    /// True only when the connection queue accepts the line; delivery can still fail later.
    pub fn send(&self, line: String) -> bool {
        if self.control.closing.load(Ordering::Acquire) {
            return false;
        }
        self.outgoing.send(Outgoing::Line(line)).is_ok()
    }

    /// Shuts the socket down; the connection thread then ends with `NetEvent::Closed`.
    pub fn close(&self) {
        self.control.cancel();
        let _ = self.outgoing.send(Outgoing::Close);
    }
}

/// A `passw` command, including the server's recursive `:` and `;` command prefixes.
pub fn is_secret(line: &str) -> bool {
    line.split(['\r', '\n']).any(|line| command_words(line).next() == Some("passw"))
}

/// GGS splits on C `isspace`; standalone `:`/`;` words recurse into the command.
/// Prefixes attached to a word and server aliases remain distinct, unknown commands.
pub(crate) fn command_words(line: &str) -> impl Iterator<Item = &str> {
    line.split([' ', '\t', '\n', '\x0B', '\x0C', '\r'])
        .filter(|word| !word.is_empty())
        .skip_while(|word| *word == ":" || *word == ";")
}

/// GGS takes 2 to 8 characters; an unregistered name logs in with an empty password.
pub fn check_login(login: &str, password: &str) -> Result<(), String> {
    if !(2..=8).contains(&login.chars().count()) {
        return Err("login must be 2 to 8 characters".into());
    }
    if login.chars().chain(password.chars()).any(|c| c.is_control() || c.is_whitespace()) {
        return Err("login and password must not contain spaces or control characters".into());
    }
    Ok(())
}

pub const KEEPALIVE: Duration = Duration::from_secs(45);
const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// `--connect HOST:PORT --login NAME [--password PW]`; `None` when `--connect` is absent.
pub fn login_from_args(args: &[String]) -> Result<Option<LoginParams>, String> {
    let value = |flag: &str| -> Result<Option<&str>, String> {
        match args.iter().position(|a| a == flag) {
            None => Ok(None),
            Some(i) => args
                .get(i + 1)
                .filter(|v| !v.starts_with("--"))
                .map(|v| Some(v.as_str()))
                .ok_or_else(|| format!("{flag} needs a value")),
        }
    };
    let Some(target) = value("--connect")? else { return Ok(None) };
    let (host, port) = target.rsplit_once(':').ok_or("--connect takes HOST:PORT")?;
    let port = port.parse::<u16>().map_err(|_| format!("bad port '{port}'"))?;
    let login = value("--login")?.ok_or("--connect needs --login")?.to_string();
    let password = value("--password")?.unwrap_or("").to_string();
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    let params = LoginParams { host: host.into(), port, login, password, keepalive: KEEPALIVE };
    params.validate()?;
    Ok(Some(params))
}

#[derive(Clone)]
pub struct LoginParams {
    pub host: String,
    pub port: u16,
    pub login: String,
    pub password: String,
    /// Idle time after which `:` is sent once logged in, so the server keeps the connection.
    pub keepalive: Duration,
}

impl LoginParams {
    pub fn validate(&self) -> Result<(), String> {
        check_login(&self.login, &self.password)?;
        if self.host.is_empty() || self.host.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err("host must not be empty or contain whitespace or control characters".into());
        }
        if self.port == 0 {
            return Err("port must be between 1 and 65535".into());
        }
        if self.keepalive.is_zero() || self.keepalive.checked_mul(3).is_none() {
            return Err("keepalive interval is out of range".into());
        }
        Ok(())
    }
}

/// Splits bytes on CR LF. A CR not followed by LF is dropped.
#[derive(Default)]
struct CrlfDecoder {
    buf: Vec<u8>,
    pending_cr: bool,
}

impl CrlfDecoder {
    fn feed(&mut self, b: u8) -> io::Result<Option<String>> {
        if std::mem::take(&mut self.pending_cr) && b == b'\n' {
            let line = String::from_utf8_lossy(&self.buf).into_owned();
            self.buf.clear();
            return Ok(Some(line));
        }
        if b == b'\r' {
            self.pending_cr = true;
        } else {
            if self.buf.len() >= MAX_LINE_BYTES {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "server line exceeds 64 KiB"));
            }
            self.buf.push(b);
        }
        Ok(None)
    }
}

/// Groups lines into messages terminated by a `READY` or `ALERT` line (spec 2.1).
#[derive(Default)]
pub struct Assembler {
    lines: Vec<String>,
    buffered_bytes: usize,
}

impl Assembler {
    pub fn feed(&mut self, line: String) -> io::Result<Option<Vec<String>>> {
        if line == "READY" || line == "ALERT" {
            let mut msg = std::mem::take(&mut self.lines);
            self.buffered_bytes = 0;
            if msg.is_empty() {
                return Ok(None);
            }
            let continuation = !msg[0].starts_with(':');
            for (i, l) in msg.iter_mut().enumerate() {
                if continuation && i > 0 && l.starts_with('|') {
                    l.remove(0);
                }
                l.retain(|c| c != '\u{7}');
            }
            return Ok(Some(msg));
        }
        if !(self.lines.is_empty() && line.is_empty()) {
            let bytes = line.len().saturating_add(std::mem::size_of::<String>());
            if bytes > MAX_MESSAGE_BYTES - self.buffered_bytes {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "server message exceeds 4 MiB"));
            }
            self.buffered_bytes += bytes;
            self.lines.push(line);
        }
        Ok(None)
    }
}

enum HandshakePhase {
    AwaitPrompt,
    Prompted,
    PasswordSent,
    Done,
}

enum Step {
    Wait,
    Send(String),
    Done,
    Fail(String),
}

/// The server turned the login down. It rides in `run`'s `io::Error` so that no socket error,
/// whatever its kind, is mistaken for it.
#[derive(Debug)]
struct LoginRejected(String);

impl std::fmt::Display for LoginRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LoginRejected {}

/// Login dialogue of spec 1.3, driven one raw line at a time.
struct Handshake {
    login: String,
    password: String,
    phase: HandshakePhase,
}

impl Handshake {
    fn new(login: String, password: String) -> Handshake {
        Handshake { login, password, phase: HandshakePhase::AwaitPrompt }
    }

    fn on_line(&mut self, line: &str) -> Step {
        match self.phase {
            HandshakePhase::Done => Step::Wait,
            _ if line.ends_with("use).") => {
                if !matches!(self.phase, HandshakePhase::AwaitPrompt) {
                    return Step::Fail("illegal login name".into());
                }
                self.phase = HandshakePhase::Prompted;
                Step::Send(self.login.clone())
            }
            // A new name is asked for a password too and takes an empty line (live server behavior).
            _ if line.contains("password") && !matches!(self.phase, HandshakePhase::AwaitPrompt) => {
                if matches!(self.phase, HandshakePhase::PasswordSent) {
                    return Step::Fail("password rejected".into());
                }
                self.phase = HandshakePhase::PasswordSent;
                Step::Send(self.password.clone())
            }
            _ if line == "READY" && !matches!(self.phase, HandshakePhase::AwaitPrompt) => {
                self.phase = HandshakePhase::Done;
                Step::Done
            }
            _ => Step::Wait,
        }
    }
}

/// Masks password command acknowledgments and finger fields before dispatch.
fn mask_secrets(line: String, in_profile: &mut bool) -> String {
    match line.as_str() {
        ": finger" => *in_profile = true,
        "READY" | "ALERT" => *in_profile = false,
        _ => {}
    }
    if line.starts_with(": +ack:") && line.split_whitespace().any(|w| w == "passw") {
        ": +ack:  passw ********".to_string()
    } else if *in_profile
        && let Some((key, _)) = line.split_once(':')
        && key.trim().trim_start_matches('|').trim() == "passw"
    {
        format!("{key}: ********")
    } else {
        line
    }
}

/// Queues the built-in commands before publishing the login to the UI, so they precede every
/// command the UI sends.
fn complete_login(tx: &mpsc::Sender<Outgoing>, events: &async_channel::Sender<NetEvent>, logged_in: &AtomicBool) {
    // Acks show what the server ran after its aliases, which `Lobby::acked` reads.
    for c in ["verbose +ack", "vt100 -", "notify + *", "notify +"] {
        let _ = tx.send(Outgoing::Line(c.into()));
    }
    logged_in.store(true, Ordering::Relaxed);
    let _ = events.send_blocking(NetEvent::LoggedIn);
}

pub fn connect(params: LoginParams, events: async_channel::Sender<NetEvent>) -> Link {
    let (tx, rx) = mpsc::channel();
    let control = Arc::new(ConnectionControl::default());
    let link = Link { outgoing: tx.clone(), control: control.clone() };
    thread::spawn(move || {
        let result = run(params, &events, tx, rx, control.clone());
        control.stream.lock().unwrap_or_else(|e| e.into_inner()).take();
        let _ = events.send_blocking(closed_event(control.cancelled.load(Ordering::Acquire), result));
    });
    link
}

/// Why the connection ended and whether to retry: a rejected login or a cancel is final.
fn closed_event(cancelled: bool, result: io::Result<()>) -> NetEvent {
    let (reason, retry) = if cancelled {
        ("disconnected".to_string(), false)
    } else {
        match result {
            Ok(()) => ("server closed the connection".to_string(), true),
            Err(e) => {
                let rejected = e.get_ref().is_some_and(|inner| inner.is::<LoginRejected>());
                (e.to_string(), !rejected)
            }
        }
    };
    NetEvent::Closed { reason, retry }
}

fn run(
    p: LoginParams,
    events: &async_channel::Sender<NetEvent>,
    tx: mpsc::Sender<Outgoing>,
    rx: mpsc::Receiver<Outgoing>,
    control: Arc<ConnectionControl>,
) -> io::Result<()> {
    p.validate().map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let status = |s: String| {
        let _ = events.send_blocking(NetEvent::Status(s));
    };
    if control.cancelled.load(Ordering::Acquire) || events.is_closed() {
        return Ok(());
    }
    status(format!("connecting to {}:{}", p.host, p.port));
    let mut last_error = io::Error::new(io::ErrorKind::AddrNotAvailable, "host has no socket addresses");
    let mut stream = None;
    for address in (p.host.as_str(), p.port).to_socket_addrs()? {
        if control.cancelled.load(Ordering::Acquire) || events.is_closed() {
            return Ok(());
        }
        match TcpStream::connect_timeout(&address, Duration::from_secs(10)) {
            Ok(connected) => {
                stream = Some(connected);
                break;
            }
            Err(e) => last_error = e,
        }
    }
    let stream = Arc::new(stream.ok_or(last_error)?);
    if !control.attach(stream.clone()) || events.is_closed() {
        return Ok(());
    }
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    stream.set_read_timeout(Some((p.keepalive / 3).max(Duration::from_millis(10))))?;
    status("connected, logging in".into());
    // One handle for both threads: on Windows a `try_clone` duplicate can report itself not connected
    // under load, so its shutdown failed and never woke the reader.
    let writer = stream.clone();
    let logged_in = Arc::new(AtomicBool::new(false));
    let keepalive = p.keepalive;
    let writer_thread = thread::spawn({
        let (logged_in, control) = (logged_in.clone(), control.clone());
        move || write_loop(writer, logged_in, &control.closing, || rx.recv_timeout(keepalive))
    });

    let result = (|| {
        let mut handshake = Handshake::new(p.login, p.password);
        let mut decoder = CrlfDecoder::default();
        let mut assembler = Assembler::default();
        let mut in_profile = false;
        let mut buf = [0u8; 4096];
        // A server that stops answering but keeps the socket open would look connected forever. Every
        // keepalive gets a reply, so three silent keepalive intervals mean the server is gone. Before
        // login no keepalive runs, so the login dialogue gets at least 60 s.
        let stall =
            |logged_in: bool| if logged_in { keepalive * 3 } else { (keepalive * 3).max(Duration::from_secs(60)) };
        let mut last_rx = Instant::now();
        loop {
            if control.cancelled.load(Ordering::Acquire) || events.is_closed() {
                return Ok(());
            }
            let n = match (&*stream).read(&mut buf) {
                Ok(n) => n,
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                    if events.is_closed() {
                        return Ok(());
                    }
                    let limit = stall(logged_in.load(Ordering::Relaxed));
                    if last_rx.elapsed() > limit {
                        return Err(io::Error::other(format!("no reply from the server for {} s", limit.as_secs())));
                    }
                    continue;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            last_rx = Instant::now();
            if n == 0 {
                return Ok(());
            }
            for &byte in &buf[..n] {
                let Some(line) = decoder.feed(byte)? else { continue };
                if control.cancelled.load(Ordering::Acquire) {
                    return Ok(());
                }
                let line = mask_secrets(line, &mut in_profile);
                if !matches!(handshake.phase, HandshakePhase::Done) {
                    // The banner only becomes a message at READY; show it live so a stalled login is visible.
                    status(line.clone());
                }
                match handshake.on_line(&line) {
                    Step::Wait => {}
                    Step::Send(line) => {
                        let _ = tx.send(Outgoing::Line(line));
                    }
                    Step::Done => complete_login(&tx, events, &logged_in),
                    Step::Fail(e) => return Err(io::Error::other(LoginRejected(e))),
                }
                if let Some(msg) = assembler.feed(line)?
                    && events.send_blocking(NetEvent::Message(msg)).is_err()
                {
                    return Ok(());
                }
            }
        }
    })();
    finish_connection(&stream, &tx, result, writer_thread, &control.closing)
}

fn finish_connection(
    stream: &TcpStream,
    tx: &mpsc::Sender<Outgoing>,
    result: io::Result<()>,
    writer_thread: thread::JoinHandle<(io::Result<()>, bool)>,
    closing: &AtomicBool,
) -> io::Result<()> {
    // Wake both threads on every exit path, even while the UI still holds a Link.
    closing.store(true, Ordering::Release);
    let _ = stream.shutdown(Shutdown::Both);
    let _ = tx.send(Outgoing::Close);
    let (writer_result, failed_first) =
        writer_thread.join().map_err(|_| io::Error::other("connection writer thread panicked"))?;
    if failed_first { writer_result } else { result }
}

/// Sends the lines `receive` yields, and a `:` keepalive when it times out once logged in. Tests pass their
/// own `receive` to trigger idle intervals without a wall-clock race.
fn write_loop(
    writer: Arc<TcpStream>,
    logged_in: Arc<AtomicBool>,
    closing: &AtomicBool,
    mut receive: impl FnMut() -> Result<Outgoing, mpsc::RecvTimeoutError>,
) -> (io::Result<()>, bool) {
    let result = (|| loop {
        let out = match receive() {
            Ok(out) => out,
            Err(mpsc::RecvTimeoutError::Timeout) if logged_in.load(Ordering::Relaxed) => Outgoing::Line(":".into()),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        };
        if closing.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut line = match out {
            Outgoing::Line(l) => l,
            Outgoing::Close => return Ok(()),
        };
        line.push('\n');
        // A raw TcpStream: no buffer to flush.
        (&*writer).write_all(line.as_bytes())?;
    })();
    // Claim a send failure before waking the reader; cleanup failures keep its original result.
    let failed_first = result.is_err() && !closing.swap(true, Ordering::AcqRel);
    let _ = writer.shutdown(Shutdown::Both);
    (result, failed_first)
}

/// Scripted server output; a line `~` means one second passes.
const DEMO: &str = include_str!("demo.txt");

/// Feeds the fixture through the same assembler and event path as a real connection.
pub fn demo(events: async_channel::Sender<NetEvent>) -> Link {
    let (tx, rx) = mpsc::channel();
    // Demo commands are accepted and discarded until the UI closes or drops the link.
    thread::spawn(move || {
        while let Ok(out) = rx.recv() {
            if matches!(out, Outgoing::Close) {
                break;
            }
        }
    });
    thread::spawn(move || {
        let _ = events.send_blocking(NetEvent::Status("demo mode: scripted server output".into()));
        let _ = events.send_blocking(NetEvent::LoggedIn);
        let mut assembler = Assembler::default();
        for line in DEMO.lines() {
            if line == "~" {
                thread::sleep(Duration::from_secs(1));
            } else {
                match assembler.feed(line.to_string()) {
                    Ok(Some(msg)) => {
                        if events.send_blocking(NetEvent::Message(msg)).is_err() {
                            return;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let _ = events.send_blocking(NetEvent::Closed { reason: e.to_string(), retry: false });
                        return;
                    }
                }
            }
        }
        let _ = events.send_blocking(NetEvent::Status("demo script finished".into()));
    });
    Link { outgoing: tx, control: Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{self, ServerMsg};
    use crate::{game::Phase, lobby::Lobby};
    use std::future::Future;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
    use std::task::{Context, Poll, Wake, Waker};

    const TEST_TIMEOUT: Duration = Duration::from_secs(10);

    fn deadline() -> Instant {
        Instant::now() + TEST_TIMEOUT
    }

    fn remaining(deadline: Instant) -> Duration {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .expect("network test exceeded its deadline")
    }

    struct WakeThread(thread::Thread);

    impl Wake for WakeThread {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }

    /// Poll an async-channel receive with a deadline; wakeups retain their token before parking.
    fn event(rx: &async_channel::Receiver<NetEvent>, deadline: Instant) -> NetEvent {
        let mut receive = std::pin::pin!(rx.recv());
        let waker = Waker::from(Arc::new(WakeThread(thread::current())));
        let mut cx = Context::from_waker(&waker);
        loop {
            let wait = remaining(deadline);
            match receive.as_mut().poll(&mut cx) {
                Poll::Ready(result) => return result.expect("network event channel closed before the expected event"),
                Poll::Pending => thread::park_timeout(wait),
            }
        }
    }

    fn receive<T>(rx: &mpsc::Receiver<T>, deadline: Instant) -> T {
        rx.recv_timeout(remaining(deadline)).expect("network test did not receive its expected notification")
    }

    /// Completion travels over a timed channel, so a broken worker cannot hang a thread join.
    fn worker<T: Send + 'static>(task: impl FnOnce() -> T + Send + 'static) -> mpsc::Receiver<thread::Result<T>> {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(catch_unwind(AssertUnwindSafe(task)));
        });
        rx
    }

    fn completed<T>(worker: &mpsc::Receiver<thread::Result<T>>, deadline: Instant) -> T {
        match receive(worker, deadline) {
            Ok(value) => value,
            Err(panic) => resume_unwind(panic),
        }
    }

    fn socket(stream: TcpStream, deadline: Instant) -> TcpStream {
        stream.set_nonblocking(false).unwrap();
        stream.set_read_timeout(Some(remaining(deadline))).unwrap();
        stream.set_write_timeout(Some(remaining(deadline))).unwrap();
        stream
    }

    fn accept(listener: &TcpListener, deadline: Instant) -> TcpStream {
        listener.set_nonblocking(true).unwrap();
        loop {
            match listener.accept() {
                Ok((stream, _)) => return socket(stream, deadline),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::park_timeout(remaining(deadline).min(Duration::from_millis(10)))
                }
                Err(e) => panic!("loopback accept failed: {e}"),
            }
        }
    }

    struct TestLink(Link);

    impl std::ops::Deref for TestLink {
        type Target = Link;
        fn deref(&self) -> &Link {
            &self.0
        }
    }

    impl Drop for TestLink {
        fn drop(&mut self) {
            self.0.close();
        }
    }

    fn test_connect(params: LoginParams, events: async_channel::Sender<NetEvent>) -> TestLink {
        TestLink(connect(params, events))
    }

    /// A login as `tester` on a loopback server.
    fn local(port: u16, password: &str, keepalive: Duration) -> LoginParams {
        LoginParams { host: "127.0.0.1".into(), port, login: "tester".into(), password: password.into(), keepalive }
    }

    fn feed_all(lines: &[&str]) -> Vec<Vec<String>> {
        let mut a = Assembler::default();
        lines.iter().filter_map(|l| a.feed(l.to_string()).unwrap()).collect()
    }

    #[test]
    fn crlf_ends_a_line_while_a_lone_cr_is_dropped_and_a_lone_lf_kept() {
        let mut d = CrlfDecoder::default();
        let out: Vec<String> = b"ab\r\ncd\rx\ne\r\n".iter().filter_map(|&b| d.feed(b).unwrap()).collect();
        assert_eq!(out, ["ab", "cdx\ne"], "lone CR dropped, lone LF kept");
    }

    #[test]
    fn incomplete_lines_and_messages_have_bounded_buffers() {
        let mut d = CrlfDecoder::default();
        for _ in 0..MAX_LINE_BYTES {
            assert!(d.feed(b'x').unwrap().is_none());
        }
        assert_eq!(d.feed(b'x').unwrap_err().kind(), io::ErrorKind::InvalidData);
        d.feed(b'\r').unwrap();
        assert_eq!(d.feed(b'\n').unwrap().unwrap().len(), MAX_LINE_BYTES);

        let mut a = Assembler::default();
        let line = "x".repeat(MAX_LINE_BYTES);
        while a.buffered_bytes + line.len() + std::mem::size_of::<String>() <= MAX_MESSAGE_BYTES {
            assert!(a.feed(line.clone()).unwrap().is_none());
        }
        assert_eq!(a.feed(line).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(a.feed("READY".into()).unwrap().is_some());
        assert_eq!(a.buffered_bytes, 0);
        assert!(a.feed("next message".into()).unwrap().is_none());
        assert_eq!(a.feed("ALERT".into()).unwrap().unwrap(), ["next message"]);
    }

    #[test]
    fn ready_or_alert_ends_a_message_and_continuation_pipes_are_stripped() {
        let msgs = feed_all(&["", "", "/os: join .1", "|(;a", "", "|b\u{7}", "READY", "READY", "x", "ALERT"]);
        assert_eq!(msgs, [vec!["/os: join .1", "(;a", "", "b"], vec!["x"]]);
    }

    #[test]
    fn assembler_keeps_pipe_after_colon_header() {
        let msgs = feed_all(&[": + alice", "|keep", "READY"]);
        assert_eq!(msgs, [vec![": + alice", "|keep"]]);
        let msgs = feed_all(&["|first", "|second", "READY"]);
        assert_eq!(msgs, [vec!["|first", "second"]], "header line is never stripped");
    }

    #[test]
    fn closed_event_retries_only_a_dropped_connection() {
        let retry = |cancelled, result| match closed_event(cancelled, result) {
            NetEvent::Closed { retry, .. } => retry,
            _ => unreachable!(),
        };
        let rejected = || Err(io::Error::other(LoginRejected("password rejected".into())));
        assert!(retry(false, Ok(())), "server closed");
        assert!(retry(false, Err(io::ErrorKind::ConnectionReset.into())), "dropped");
        assert!(retry(false, Err(io::ErrorKind::PermissionDenied.into())), "a socket's own error is not a rejection");
        assert!(!retry(false, rejected()), "login rejected");
        assert!(!retry(true, Ok(())) && !retry(true, rejected()), "cancelled");
        assert!(matches!(closed_event(true, Ok(())), NetEvent::Closed { reason, .. } if reason == "disconnected"));
    }

    fn sent(step: Step) -> Option<String> {
        match step {
            Step::Send(l) => Some(l),
            _ => None,
        }
    }

    #[test]
    fn handshake_sends_login_then_password() {
        let mut h = Handshake::new("me".into(), "pw".into());
        assert!(sent(h.on_line("welcome")).is_none());
        assert_eq!(sent(h.on_line(": Enter login (yours, or one you'd like to use).")), Some("me".into()));
        assert_eq!(sent(h.on_line(": Enter your password.")), Some("pw".into()));
        assert!(matches!(h.on_line("READY"), Step::Done));
        assert!(matches!(h.on_line("READY"), Step::Wait), "done once");
    }

    #[test]
    fn only_a_ready_line_after_the_prompt_completes_login() {
        let mut h = Handshake::new("me".into(), String::new());
        assert!(matches!(h.on_line("READY"), Step::Wait));
        h.on_line("... use).");
        assert!(matches!(h.on_line("not READY yet"), Step::Wait));
        assert!(matches!(h.on_line("READY"), Step::Done));
    }

    #[test]
    fn handshake_rejects_second_prompt() {
        let mut h = Handshake::new("bad name".into(), String::new());
        h.on_line("... use).");
        assert!(matches!(h.on_line("... use)."), Step::Fail(_)));
        let mut h = Handshake::new("me".into(), String::new());
        h.on_line("... use).");
        assert_eq!(
            sent(h.on_line(": Enter your password.")),
            Some(String::new()),
            "an empty password is still answered"
        );
        assert!(matches!(h.on_line(": Enter your password."), Step::Fail(_)), "asked twice means rejected");
    }

    #[test]
    fn initial_commands_are_queued_before_the_ui_can_handle_logged_in() {
        let deadline = deadline();
        let (tx, rx) = mpsc::channel();
        let (events, received) = async_channel::bounded(1);
        // Keep event publication blocked to expose an event-before-commands order
        // deterministically, without relying on the two threads' scheduling.
        assert!(events.try_send(NetEvent::Status("pending".into())).is_ok());
        let logged_in = Arc::new(AtomicBool::new(false));
        let login_thread = worker({
            let (tx, logged_in) = (tx.clone(), logged_in.clone());
            move || complete_login(&tx, &events, &logged_in)
        });
        for expected in ["verbose +ack", "vt100 -", "notify + *", "notify +"] {
            match receive(&rx, deadline) {
                Outgoing::Line(line) => assert_eq!(line, expected),
                _ => panic!("expected an initial command"),
            }
        }
        assert!(login_thread.try_recv().is_err(), "LoggedIn still waits for room in the event queue");
        assert!(matches!(event(&received, deadline), NetEvent::Status(_)));
        assert!(matches!(event(&received, deadline), NetEvent::LoggedIn));
        assert!(logged_in.load(Ordering::Relaxed), "keepalives are enabled once the login is published");
        Link { outgoing: tx, control: Default::default() }.send("notify - *".into());
        assert!(matches!(receive(&rx, deadline), Outgoing::Line(line) if line == "notify - *"));
        completed(&login_thread, deadline);
    }

    #[test]
    fn loopback_login_logs_raw_lines_without_password() {
        let deadline = deadline();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = worker(move || {
            let mut s = accept(&listener, deadline);
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut got = Vec::new();
            s.write_all(b": Enter login (yours, or one you'd like to use).\r\n").unwrap();
            for prompt in [&b": Enter your password.\r\n"[..], b"READY\r\n/os: match\r\n|.1 x\r\nREADY\r\n"] {
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                got.push(line);
                s.write_all(prompt).unwrap();
            }
            // Drain the post-login commands so closing does not reset the connection with unread data.
            for _ in 0..4 {
                r.read_line(&mut String::new()).unwrap();
            }
            s.write_all(format!(": finger\r\nlogin  : tester\r\npassw  : pw-{port}\r\n: finger\r\nlogin  : other\r\npassw  : admin-pw-{port}\r\n: +ack:  finger tester other => finger tester other\r\nREADY\r\npassw: hello\r\n|passw  : quoted in a chat\r\nREADY\r\n").as_bytes()).unwrap();
            got
        });
        let (tx, rx) = async_channel::unbounded();
        let secret = format!("pw-{}", port);
        let _link = test_connect(local(port, &secret, KEEPALIVE), tx);
        let mut logged_in = false;
        let mut messages = Vec::new();
        loop {
            match event(&rx, deadline) {
                NetEvent::LoggedIn => logged_in = true,
                NetEvent::Message(m) => messages.push(m),
                NetEvent::Closed { .. } => break,
                NetEvent::Status(_) => {}
            }
        }
        assert_eq!(completed(&server, deadline), ["tester\n", format!("{secret}\n").as_str()], "LF-terminated writes");
        assert!(logged_in);
        assert!(messages.contains(&vec!["/os: match".to_string(), ".1 x".to_string()]));
        assert!(messages.contains(&vec!["passw: hello".to_string(), "passw  : quoted in a chat".to_string()]));
        assert!(messages.iter().flatten().any(|l| l == "passw  : ********"));
        for password in [&secret, &format!("admin-pw-{port}")] {
            assert!(
                messages.iter().flatten().all(|l| !l.contains(password)),
                "password must never reach message dispatch"
            );
        }
    }

    #[test]
    fn keepalive_only_after_login_without_waiting_for_wall_time() {
        let deadline = deadline();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let writer =
            socket(TcpStream::connect_timeout(&listener.local_addr().unwrap(), remaining(deadline)).unwrap(), deadline);
        let server = accept(&listener, deadline);
        let mut reader = BufReader::new(server);
        let (tx, rx) = mpsc::channel();
        let logged_in = Arc::new(AtomicBool::new(false));
        let writer_thread = worker({
            let logged_in = logged_in.clone();
            move || write_loop(Arc::new(writer), logged_in, &AtomicBool::new(false), || receive(&rx, deadline))
        });

        // Queue a timeout and a marker. Seeing the marker confirms that the writer
        // processed the idle interval while login was pending without sending ':'.
        tx.send(Err(mpsc::RecvTimeoutError::Timeout)).unwrap();
        tx.send(Ok(Outgoing::Line("before-login".into()))).unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "before-login\n", "no keepalive while the login prompt is pending");

        logged_in.store(true, Ordering::Relaxed);
        tx.send(Err(mpsc::RecvTimeoutError::Timeout)).unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, ":\n", "an idle interval after login sends a keepalive");

        tx.send(Ok(Outgoing::Close)).unwrap();
        assert_eq!(reader.read_line(&mut String::new()).unwrap(), 0, "Close shuts down the writer's socket");
        let (result, failed_first) = completed(&writer_thread, deadline);
        result.unwrap();
        assert!(!failed_first);
    }

    #[test]
    fn close_ends_the_connection_after_the_server_confirms_login() {
        let deadline = deadline();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (ready_tx, ready_rx) = mpsc::channel();
        let server = worker(move || {
            let mut s = accept(&listener, deadline);
            let mut r = BufReader::new(s.try_clone().unwrap());
            s.write_all(b": Enter login (yours, or one you'd like to use).\r\n").unwrap();
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            assert_eq!(line, "tester\n");
            s.write_all(b"READY\r\n").unwrap();
            for expected in ["verbose +ack\n", "vt100 -\n", "notify + *\n", "notify +\n"] {
                line.clear();
                r.read_line(&mut line).unwrap();
                assert_eq!(line, expected);
            }
            ready_tx.send(()).unwrap();
            assert_eq!(
                r.read_line(&mut String::new()).unwrap(),
                0,
                "Close releases the connection while the link is owned"
            );
        });
        let (tx, rx) = async_channel::unbounded();
        let link = test_connect(local(port, "", KEEPALIVE), tx);
        while !matches!(event(&rx, deadline), NetEvent::LoggedIn) {}
        receive(&ready_rx, deadline);
        link.close();
        while !matches!(event(&rx, deadline), NetEvent::Closed { .. }) {}
        completed(&server, deadline);
    }

    #[test]
    fn cancel_before_connect_never_opens_a_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let (events, _rx) = async_channel::unbounded();
        let (tx, rx) = mpsc::channel();
        let control = Arc::new(ConnectionControl::default());
        let link = Link { outgoing: tx.clone(), control: control.clone() };
        link.close();
        assert!(!link.send("must not be queued".into()));
        assert!(matches!(rx.try_recv(), Ok(Outgoing::Close)));
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        run(local(listener.local_addr().unwrap().port(), "", KEEPALIVE), &events, tx, rx, control).unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), io::ErrorKind::WouldBlock);
    }

    #[test]
    fn cancellation_before_socket_attachment_closes_the_late_socket() {
        let deadline = deadline();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = Arc::new(socket(
            TcpStream::connect_timeout(&listener.local_addr().unwrap(), remaining(deadline)).unwrap(),
            deadline,
        ));
        let mut server = accept(&listener, deadline);
        let control = ConnectionControl::default();
        control.cancel();
        assert!(!control.attach(stream));
        assert_eq!(server.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn cancelling_a_silent_login_closes_without_retry_or_sending_credentials() {
        let deadline = deadline();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let server = worker(move || {
            let mut stream = accept(&listener, deadline);
            accepted_tx.send(()).unwrap();
            assert_eq!(stream.read(&mut [0]).unwrap(), 0, "cancel interrupts a stalled login");
        });
        let (tx, rx) = async_channel::unbounded();
        let link = test_connect(local(port, "test-secret", KEEPALIVE), tx);
        receive(&accepted_rx, deadline);
        link.close();
        loop {
            match event(&rx, deadline) {
                NetEvent::Closed { reason, retry } => {
                    assert_eq!((reason.as_str(), retry), ("disconnected", false));
                    break;
                }
                NetEvent::LoggedIn => panic!("a cancelled login must not complete"),
                _ => {}
            }
        }
        completed(&server, deadline);
    }

    #[test]
    fn silent_server_is_detected_as_gone() {
        let deadline = deadline();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (hold_tx, hold_rx) = mpsc::channel::<()>();
        let server = worker(move || {
            let mut s = accept(&listener, deadline);
            s.write_all(b": Enter login (yours, or one you'd like to use).\r\nREADY\r\n").unwrap();
            // Keep the socket open and never answer again, like a hung server.
            receive(&hold_rx, deadline);
        });
        let (tx, rx) = async_channel::unbounded();
        let _link = test_connect(local(port, "", Duration::from_millis(100)), tx);
        let reason = loop {
            if let NetEvent::Closed { reason, .. } = event(&rx, deadline) {
                break reason;
            }
        };
        hold_tx.send(()).unwrap();
        completed(&server, deadline);
        assert!(reason.starts_with("no reply from the server"), "{reason}");
    }

    /// Runs one connection against a loopback server and returns how it closed.
    fn closed_by(server: impl FnOnce(TcpStream) + Send + 'static) -> (String, bool) {
        let deadline = deadline();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = worker(move || server(accept(&listener, deadline)));
        let (tx, rx) = async_channel::unbounded();
        let _link = test_connect(local(port, "pw", KEEPALIVE), tx);
        loop {
            if let NetEvent::Closed { reason, retry } = event(&rx, deadline) {
                completed(&server, deadline);
                return (reason, retry);
            }
        }
    }

    #[test]
    fn a_rejected_login_is_final_and_a_dropped_connection_is_retried() {
        let (reason, retry) = closed_by(|mut s| {
            let mut r = BufReader::new(s.try_clone().unwrap());
            s.write_all(b": Enter login (yours, or one you'd like to use).\r\n: Enter your password.\r\n").unwrap();
            r.read_line(&mut String::new()).unwrap();
            r.read_line(&mut String::new()).unwrap();
            s.write_all(b": Enter your password.\r\n").unwrap();
            let _ = r.read_line(&mut String::new());
        });
        assert_eq!((reason.as_str(), retry), ("password rejected", false), "a password rejection is not retried");
        let (reason, retry) = closed_by(|mut s| {
            let mut r = BufReader::new(s.try_clone().unwrap());
            s.write_all(b": Enter login (yours, or one you'd like to use).\r\nREADY\r\n").unwrap();
            // The login and all four post-login commands, so closing does not reset the connection.
            for _ in 0..5 {
                r.read_line(&mut String::new()).unwrap();
            }
        });
        assert_eq!((reason.as_str(), retry), ("server closed the connection", true), "a server EOF is retried");
    }

    #[test]
    fn queued_writes_after_reader_eof_preserve_normal_close() {
        let deadline = deadline();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = Arc::new(socket(
            TcpStream::connect_timeout(&listener.local_addr().unwrap(), remaining(deadline)).unwrap(),
            deadline,
        ));
        let server = accept(&listener, deadline);
        server.shutdown(Shutdown::Write).unwrap();
        assert_eq!((&*stream).read(&mut [0]).unwrap(), 0, "the server closed normally");

        let writer = stream.clone();
        let closing = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let writer_thread = thread::spawn({
            let closing = closing.clone();
            move || {
                // Delay the queued writes until the reader shuts down the socket.
                assert!(matches!(receive(&rx, deadline), Outgoing::Close));
                let (queued_tx, queued_rx) = mpsc::channel();
                for line in ["vt100 -", "notify + *", "notify +", ":"] {
                    queued_tx.send(Outgoing::Line(line.into())).unwrap();
                }
                let result =
                    write_loop(writer, Arc::new(AtomicBool::new(true)), &closing, || queued_rx.recv_timeout(KEEPALIVE));
                assert!(result.0.is_ok(), "queued writes are discarded after reader shutdown");
                assert!(!result.1, "no cleanup write can replace the reader result");
                result
            }
        });
        let finish = worker(move || finish_connection(&stream, &tx, Ok(()), writer_thread, &closing));
        completed(&finish, deadline).expect("cleanup writes must not turn EOF into an error");
        let mut received = Vec::new();
        (&server).read_to_end(&mut received).unwrap();
        assert!(received.is_empty(), "none of the queued commands reached the server");
    }

    #[test]
    fn writer_failure_before_reader_finishes_is_reported() {
        let deadline = deadline();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = Arc::new(socket(
            TcpStream::connect_timeout(&listener.local_addr().unwrap(), remaining(deadline)).unwrap(),
            deadline,
        ));
        let server = accept(&listener, deadline);
        let writer = stream.clone();
        writer.shutdown(Shutdown::Write).unwrap();
        let closing = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        tx.send(Outgoing::Line(":".into())).unwrap();
        let (failed_tx, failed_rx) = mpsc::channel();
        let writer_thread = thread::spawn({
            let closing = closing.clone();
            move || {
                let result =
                    write_loop(writer, Arc::new(AtomicBool::new(true)), &closing, || rx.recv_timeout(KEEPALIVE));
                assert!(result.0.is_err());
                assert!(result.1, "the send failed before it shut down the reader");
                failed_tx.send(()).unwrap();
                result
            }
        });
        receive(&failed_rx, deadline);
        server.shutdown(Shutdown::Write).unwrap();
        let reader_result = (&*stream).read(&mut [0]).map(|_| ());
        let finish = worker(move || finish_connection(&stream, &tx, reader_result, writer_thread, &closing));
        let error = completed(&finish, deadline).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn rejected_login_closes_the_socket_while_the_link_is_still_owned() {
        let deadline = deadline();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = worker(move || {
            let mut s = accept(&listener, deadline);
            let mut r = BufReader::new(s.try_clone().unwrap());
            s.write_all(b": Enter login (yours, or one you'd like to use).\r\n: Enter your password.\r\n").unwrap();
            for _ in 0..2 {
                r.read_line(&mut String::new()).unwrap();
            }
            s.write_all(b": Enter your password.\r\n").unwrap();
            assert_eq!(r.read_line(&mut String::new()).unwrap(), 0, "failed login must release the connection");
        });
        let (tx, rx) = async_channel::unbounded();
        let link = test_connect(local(port, "pw", KEEPALIVE), tx);
        while !matches!(event(&rx, deadline), NetEvent::Closed { retry: false, .. }) {}
        completed(&server, deadline);
        drop(link);
    }

    /// Seen live: `passw zzsecret1 zzold2` was acked as `: +ack:  passw zzsecret1 zzold2 => passw zzsecret1 zzold2`.
    #[test]
    fn password_acks_and_profile_fields_are_masked() {
        let mut in_profile = false;
        let mut mask = |line: &str| mask_secrets(line.to_string(), &mut in_profile);
        let masked = mask(": +ack:  passw zzsecret1 zzold2 => passw zzsecret1 zzold2");
        assert!(!masked.contains("zzsecret1") && !masked.contains("zzold2"), "{masked}");
        assert_eq!(mask(": +ack:  pw x => passw x y"), ": +ack:  passw ********", "an alias that expands to passw");
        for command in [": passw zzsecret1 zzold2", "; : passw zzsecret1 zzold2", ": ; : passw zzsecret1 zzold2"] {
            assert_eq!(
                mask(&format!(": +ack:  {command} => {command}")),
                ": +ack:  passw ********",
                "prefixed password commands are masked on receipt"
            );
        }
        assert_eq!(
            mask(": +ack:  pw zzsecret1 => : passw zzsecret1 zzold2"),
            ": +ack:  passw ********",
            "an alias expanding to a prefixed password command is masked"
        );
        for end in ["READY", "ALERT"] {
            assert_eq!(mask(": finger"), ": finger");
            for key in ["passw  ", "passw", "  passw\t", "|passw  ", "  | passw "] {
                assert_eq!(mask(&format!("{key}: zzsecret1")), format!("{key}: ********"));
            }
            assert_eq!(mask(end), end);
            for line in [
                ": +ack:  who => who",
                "bob: my passw is long",
                "info   : passw is a command",
                "login  : tester",
                "passw: hello",
                "|passw  : quoted in a chat",
            ] {
                assert_eq!(mask(line), line);
            }
        }
    }

    #[test]
    fn connect_flags_need_a_host_port_and_a_valid_login() {
        let args = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        let p = login_from_args(&args("--connect 127.0.0.1:5000 --login pview1")).unwrap().unwrap();
        assert_eq!((p.host.as_str(), p.port, p.login.as_str(), p.password.as_str()), ("127.0.0.1", 5000, "pview1", ""));
        let p = login_from_args(&args("--login me --password pw --connect host:4000")).unwrap().unwrap();
        assert_eq!((p.port, p.password.as_str()), (4000, "pw"));
        assert!(login_from_args(&args("--demo")).unwrap().is_none());
        assert!(login_from_args(&args("--connect host")).is_err(), "port required");
        assert!(login_from_args(&args("--connect host:x --login me")).is_err(), "non-numeric port");
        assert!(login_from_args(&args("--connect host:1")).is_err(), "login required");
        assert!(login_from_args(&args("--connect host:1 --login x")).is_err(), "login rules apply");
        assert!(login_from_args(&args("--connect")).is_err(), "--connect without a value");
        for s in [
            "--connect :5000 --login me",
            "--connect host:0 --login me",
            "--connect --login me",
            "--connect host:5000 --login --password pw",
        ] {
            assert!(login_from_args(&args(s)).is_err(), "{s}");
        }
        let p = login_from_args(&args("--connect [::1]:5000 --login me")).unwrap().unwrap();
        assert_eq!(p.host, "::1");
    }

    #[test]
    fn invalid_connection_parameters_are_rejected() {
        let mut p = local(5000, "", KEEPALIVE);
        for keepalive in [Duration::ZERO, Duration::MAX] {
            p.keepalive = keepalive;
            assert!(p.validate().is_err(), "keepalive {keepalive:?}");
        }
        p.keepalive = KEEPALIVE;
        for host in ["", "has space", "has\nnewline"] {
            p.host = host.into();
            assert!(p.validate().is_err(), "host {host:?}");
        }
        assert!(check_login("me", "has\u{a0}space").is_err(), "a non-ASCII space in the password");
    }

    #[test]
    fn only_a_passw_command_word_makes_a_line_secret() {
        for line in [
            "passw new old",
            "  passw x y",
            ": passw new old",
            "; passw new old",
            " : ; : passw new old",
            "\x0B:\t;\x0Cpassw new old",
            "who\n: passw new old",
            "who\r; passw new old",
        ] {
            assert!(is_secret(line), "{line:?}");
        }
        for line in [
            "tell bob passw is not a command here",
            ": tell bob passw new old",
            ":passw new old",
            ";passw new old",
            ":; passw new old",
            "\u{a0}passw new old",
            "passw\u{3000}new old",
            ": ;",
            "who",
        ] {
            assert!(!is_secret(line), "{line:?}: only C whitespace and standalone prefix words are commands");
        }
    }

    #[test]
    fn logins_have_two_to_eight_characters_without_spaces_or_controls() {
        assert!(check_login("viewer", "").is_ok(), "empty password for an unregistered name");
        assert!(check_login("ab", "pw").is_ok(), "two characters");
        assert!(check_login("eightchr", "").is_ok(), "eight characters");
        assert!(check_login("x", "").is_err(), "one character");
        assert!(check_login("ninechars", "").is_err(), "nine characters");
        assert!(check_login("", "").is_err(), "empty login");
        assert!(check_login("ev il", "").is_err(), "space in the login");
        assert!(check_login("view\ner", "").is_err(), "newline in the login");
        assert!(check_login("viewer", "bad\rpass").is_err(), "CR in the password");
    }

    fn result(g: &crate::game::Game) -> Vec<String> {
        match &g.phase {
            Phase::Over(o) => o.describe(),
            Phase::Live => panic!("{} is still live", g.id),
        }
    }

    /// Replays what a live GGS sent to one login, returning the lobby and the notes it produced.
    fn replay(text: &str) -> (Lobby, Vec<String>) {
        let mut a = Assembler::default();
        let mut lobby = Lobby::default();
        let mut notes = Vec::new();
        for line in text.lines() {
            if let Some(ack) = line.strip_prefix(": +ack:  ")
                && let Some((command, _)) = ack.split_once(" => ")
            {
                lobby.sent(command, crate::lobby::CommandSource::App);
            }
            if let Some(ServerMsg::Os(m)) = a.feed(line.to_string()).unwrap().map(proto::parse) {
                notes.extend(lobby.apply(m, Instant::now()));
            }
        }
        (lobby, notes)
    }

    #[test]
    fn live_normal_game() {
        let (lobby, notes) = replay(include_str!("../fixtures/live-normal.txt"));
        assert_eq!(notes, ["unknown /os message:", "/os: ERR match '.4' not found."]);
        let g = &lobby.games[".4"];
        assert_eq!((g.black.name.as_str(), g.white.name.as_str()), ("pbot", "edax"));
        assert_eq!(g.moves().len(), 48, "every update applied, passes included");
        assert_eq!(g.moves().last().map(|m| m.token.mv.to_string()).as_deref(), Some("f7"));
        assert_eq!(result(g), ["pbot -64 edax", "edax wins by 64"], "result from - match");
        assert_eq!(g.board(None).count(), (0, 46), "a wipeout; empties go to the winner in the -64");
        assert!(lobby.matches.is_empty() && lobby.watched.is_empty());
        let users: Vec<(&str, Option<&str>)> =
            lobby.users().iter().map(|u| (u.name.as_str(), u.playing.as_deref())).collect();
        assert_eq!(users, [("edax", Some(".4")), ("pbot", Some(".4")), ("pwatch", None)]);
        let edax = lobby.match_settings["edax"].as_ref().unwrap();
        assert_eq!(
            (edax.open, edax.playing, edax.accept.as_deref(), edax.decline.as_deref()),
            (Some(0), Some(1), Some("size > 0"), Some(""))
        );
        let pbot = lobby.match_settings["pbot"].as_ref().unwrap();
        assert_eq!((pbot.open, pbot.playing, pbot.accept.as_deref()), (Some(1), Some(1), Some("")));
    }

    #[test]
    fn live_synchro_game() {
        let (lobby, notes) = replay(include_str!("../fixtures/live-synchro.txt"));
        assert!(
            notes.iter().all(|n| n.starts_with("unknown")
                || n.starts_with("/os: watch")
                || n == "not watching *: no such match"),
            "{notes:?}"
        );
        let g0 = &lobby.games[".5.0"];
        let g1 = &lobby.games[".5.1"];
        assert_eq!(
            (g0.black.name.as_str(), g1.black.name.as_str()),
            ("pbot", "edax"),
            "colors swap in the second board"
        );
        assert_eq!(g0.board(None).count(), g1.board(None).count(), "same random start");
        assert_eq!(result(g0), ["pbot -64 edax", "edax wins by 64"], "result from end, kept over the match result");
        assert_eq!(g1.phase, g0.phase);
        assert!(g1.text.is_empty(), "the end line is the result, not kibitz");
        assert_eq!(lobby.watched, Vec::<String>::new());
    }

    #[test]
    fn live_notify_game() {
        let (lobby, _) = replay(include_str!("../fixtures/live-notify.txt"));
        let g = &lobby.games[".6"];
        assert_eq!((g.black.name.as_str(), g.moves().len()), ("edax", 1), "GGF move tag in capitals, B[E6]");
        assert_eq!(result(g), ["pbot -64 edax", "edax wins by 64"], "score is player 1's, not black's");
    }

    /// A second login, not watching, sent `tell /os tell .14 ...`, then `tell o`, `tell p` and `tell a`.
    /// `tell p` reaches the players only (EXE_Service.C `tell`), so the watcher never saw it.
    #[test]
    fn live_kibitz() {
        let (lobby, notes) = replay(include_str!("../fixtures/live-kibitz.txt"));
        assert!(notes.is_empty(), "{notes:?}");
        let g = &lobby.games[".14"];
        assert_eq!(g.text, ["pkib2: hello all", "pkib2: hello observers", "pkib2: hello a"]);
        assert_eq!(g.moves().len(), 12);
    }

    /// An observer while prated lost a rated game to edax, then asked for history, stored, look and top.
    #[test]
    fn live_archive_session() {
        let (lobby, notes) = replay(include_str!("../fixtures/live-archive.txt"));
        let f = lobby.finished.back().expect("the rated match is listed as finished");
        assert_eq!((f.report.summary.id.as_str(), f.report.archive.as_deref()), (".23", Some(".73")));
        assert_eq!(lobby.history["prated"].len(), 4);
        assert!(lobby.history["parch"].is_empty());
        assert_eq!(lobby.stored["prated"].iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), [".69"]);
        assert_eq!(lobby.ranks().iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["edax", "prated"]);
        assert_eq!(lobby.games["archive .73"].source, crate::game::GameSource::Archive);
        assert!(!notes.iter().any(|n| n.starts_with("look reply that was not asked for")), "{notes:?}");
        assert!(notes.contains(&".17 is not in the archive; only rated matches are kept".to_string()), "{notes:?}");
    }

    /// pchat1 talking with pchat2 on .chat, in tells and in the kibitz of .1.
    #[test]
    fn live_chat_session() {
        let text = include_str!("../fixtures/live-chat.txt");
        let mut a = Assembler::default();
        let chat: Vec<(String, String)> = text
            .lines()
            .filter_map(|l| a.feed(l.to_string()).unwrap())
            .filter_map(|m| match proto::parse(m) {
                ServerMsg::Chat { key, line } => Some((key.label(), line.lines.join("|"))),
                _ => None,
            })
            .collect();
        let want = [
            (".chat", "reply from b"),
            ("tell pchat2", "hi a"),
            ("tell pchat1", "note to self"),
            ("tell pchat2", "two|lines"),
        ];
        assert_eq!(chat, want.map(|(k, l)| (k.to_string(), l.to_string())));
        let (lobby, _) = replay(text);
        assert_eq!(
            lobby.games[".1"].text,
            ["pchat2: from b", "pchat2: b to observers"],
            "pchat1's own kibitz never came back"
        );
    }

    #[test]
    fn demo_script_replays_cleanly() {
        let mut a = Assembler::default();
        let mut lobby = Lobby::default();
        lobby.sent("tell /os who 8 a", crate::lobby::CommandSource::App);
        let mut os_seen = false;
        let now = Instant::now();
        for line in DEMO.lines().filter(|l| *l != "~") {
            let Some(msg) = a.feed(line.to_string()).unwrap() else { continue };
            match proto::parse(msg) {
                ServerMsg::Os(m) => {
                    let notes = lobby.apply(m, now);
                    assert!(notes.is_empty(), "demo message produced notes: {notes:?}");
                }
                ServerMsg::Presence { os } => os_seen |= os == Some(true),
                ServerMsg::Help(_)
                | ServerMsg::Chat { .. }
                | ServerMsg::Finger { .. }
                | ServerMsg::OsAbsent(_)
                | ServerMsg::Td(_)
                | ServerMsg::TdNews(_)
                | ServerMsg::Other(_) => {}
            }
        }
        assert!(os_seen);
        let g = &lobby.games[".12"];
        assert!(g.moves().len() >= 8);
        assert!(g.observers.contains("eve"));
        assert!(matches!(g.phase, Phase::Over(_)));
        assert!(lobby.matches.iter().all(|m| m.id() != ".12"));
        let live: Vec<&str> = lobby.games.values().filter(|g| g.phase == Phase::Live).map(|g| g.id.as_str()).collect();
        assert_eq!(live, [".15.0", ".15.1", ".23", ".25", ".27", ".29"], "concurrent games remain available");
        assert!(matches!(lobby.games[".19"].phase, Phase::Over(_)), "a game ends midway");
        assert!(lobby.games[".23"].observers.contains("dave"));
        assert_eq!(lobby.games[".23"].text, ["eve: what a squeeze on the south edge"], "kibitz shown as name: text");
        assert_eq!(lobby.games[".19"].board(None).size(), 10);
        assert_eq!(lobby.users().len(), 9, "who list without /os");
        assert_eq!(
            lobby.match_settings.values().flatten().filter(|s| s.rated == Some(true)).count(),
            4,
            "finger reply applied"
        );
        assert!(lobby.users().iter().any(|u| u.name == "ghost"));
        assert!(lobby.match_settings.get("ghost").is_none_or(|s| s.as_ref().is_none_or(|s| s.rated.is_none())));
    }
}

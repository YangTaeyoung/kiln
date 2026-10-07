//! Isolated PTY regression for an interrupted TUI synchronized redraw.
//! It never connects to the user's daemon or sends input to real agents.
#![cfg(unix)]

use kiln_daemon::client::Client;
use kiln_proto::{ClientMsg, ServerMsg, SpawnSpec, read_msg, write_msg};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A relay around an owned host lets the test interrupt only IPC, never the
/// child. Hold the next attachment before forwarding its Hello to build up real
/// detached output and queue user input behind the replacement handshake.
struct HostRelay {
    original: std::path::PathBuf,
    backend: std::path::PathBuf,
    hold: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pause_input: std::sync::Arc<std::sync::atomic::AtomicBool>,
    input_blocked: std::sync::Arc<std::sync::atomic::AtomicBool>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    sockets: std::sync::Arc<std::sync::Mutex<Vec<UnixStream>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl HostRelay {
    fn install(endpoint: &str) -> Self {
        use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
        let original=std::path::PathBuf::from(endpoint);let backend=original.with_file_name("relay-backend.sock");
        assert!(!backend.exists());std::fs::rename(&original,&backend).unwrap();
        let listener=std::os::unix::net::UnixListener::bind(&original).unwrap();listener.set_nonblocking(true).unwrap();
        let hold=Arc::new(AtomicBool::new(false));let stop=Arc::new(AtomicBool::new(false));let sockets=Arc::new(Mutex::new(Vec::<UnixStream>::new()));
        let pause_input=Arc::new(AtomicBool::new(false));let input_blocked=Arc::new(AtomicBool::new(false));
        let paused=pause_input.clone();let blocked=input_blocked.clone();
        let waiting=hold.clone();let stopping=stop.clone();let owned=sockets.clone();let target=backend.clone();
        let thread=std::thread::spawn(move||{
            let mut workers=Vec::new();
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut client,_))=>{
                        client.set_nonblocking(false).unwrap();
                        let target=target.clone();let waiting=waiting.clone();let stopping=stopping.clone();let owned=owned.clone();let paused=paused.clone();let blocked=blocked.clone();
                        workers.push(std::thread::spawn(move||{
                            owned.lock().unwrap().push(client.try_clone().unwrap());
                            while waiting.load(Ordering::SeqCst) && !stopping.load(Ordering::SeqCst) {std::thread::sleep(Duration::from_millis(5));}
                            if stopping.load(Ordering::SeqCst){return;}
                            let Ok(mut backend)=UnixStream::connect(target) else{return;};
                            {
                                let mut streams=owned.lock().unwrap();streams.push(backend.try_clone().unwrap());
                                if stopping.load(Ordering::SeqCst){let _=backend.shutdown(std::net::Shutdown::Both);return;}
                            }
                            let mut source=client.try_clone().unwrap();let mut destination=backend.try_clone().unwrap();
                            let upstream=std::thread::spawn(move||{
                                use std::io::{Read,Write};
                                let mut bytes=[0;4096];
                                while let Ok(n)=source.read(&mut bytes){
                                    if n==0{break;}
                                    while paused.load(Ordering::SeqCst) && !stopping.load(Ordering::SeqCst){blocked.store(true,Ordering::SeqCst);std::thread::sleep(Duration::from_millis(5));}
                                    if stopping.load(Ordering::SeqCst) || destination.write_all(&bytes[..n]).is_err(){break;}
                                }
                                let _=destination.shutdown(std::net::Shutdown::Write);
                            });
                            let _=std::io::copy(&mut backend,&mut client);let _=client.shutdown(std::net::Shutdown::Write);let _=upstream.join();
                        }));
                    },
                    Err(e) if e.kind()==std::io::ErrorKind::WouldBlock=>std::thread::sleep(Duration::from_millis(5)),
                    Err(_)=>break,
                }
            }
            for stream in owned.lock().unwrap().iter(){let _=stream.shutdown(std::net::Shutdown::Both);}
            for worker in workers{let _=worker.join();}
        });
        Self{original,backend,hold,pause_input,input_blocked,stop,sockets,thread:Some(thread)}
    }
    fn interrupt_and_hold(&self){
        self.hold.store(true,std::sync::atomic::Ordering::SeqCst);
        for stream in self.sockets.lock().unwrap().drain(..){let _=stream.shutdown(std::net::Shutdown::Both);}
    }
    fn release(&self){self.hold.store(false,std::sync::atomic::Ordering::SeqCst);}
}
impl Drop for HostRelay {
    fn drop(&mut self){
        self.stop.store(true,std::sync::atomic::Ordering::SeqCst);self.release();
        for stream in self.sockets.lock().unwrap().iter(){let _=stream.shutdown(std::net::Shutdown::Both);}
        if let Some(thread)=self.thread.take(){let _=thread.join();}
        let _=std::fs::remove_file(&self.original);let _=std::fs::rename(&self.backend,&self.original);
    }
}

struct Fixture {
    child: Child,
    socket: String,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn start(hosted: bool) -> (Self, Client) {
        let dir = tempfile::Builder::new().prefix("kiln-sync-").tempdir_in("/tmp").unwrap();
        Self::start_in(dir, hosted)
    }

    fn start_in(dir: tempfile::TempDir, hosted: bool) -> (Self, Client) {
        let socket = dir.path().join("daemon.sock").to_string_lossy().into_owned();
        let child = Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(["daemon", "--foreground", "--socket", &socket])
            .env("KILN_SOCKET", &socket)
            .env("KILN_PTY_HOST", if hosted { "1" } else { "0" })
            .env("KILN_CONFIG_DIR", dir.path().join("config"))
            .env("KILN_NO_AUTO_UPGRADE", "1")
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().unwrap();
        let fixture = Self { child, socket, _dir: dir };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(client) = Client::connect(&fixture.socket, None) { return (fixture, client); }
            assert!(Instant::now() < deadline, "isolated daemon did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn hosted_backpressure_case(reconnect: bool) {
    use kiln_daemon::ptyhost::{HostPty, HostRecord};
    struct OwnedHost(HostPty);
    impl Drop for OwnedHost {
        fn drop(&mut self){
            self.0.kill();
            // A regression may block the control path. Verify private process
            // identity before cleaning our detached owner/child, never a PID alone.
            let root=std::path::Path::new(&self.0.endpoint).parent().unwrap();
            if kiln_daemon::procinfo::cwd(self.0.pid()).is_some_and(|cwd|std::path::Path::new(&cwd).starts_with(root)) {
                unsafe{libc::kill(self.0.pid() as i32,libc::SIGHUP);}
            }
            if kiln_daemon::procinfo::cmdline(self.0.host_pid).is_some_and(|args|args.iter().any(|arg|arg==&self.0.endpoint)) {
                unsafe{libc::kill(self.0.host_pid as i32,libc::SIGTERM);}
            }
        }
    }
    let dir=tempfile::Builder::new().prefix("kiln-backlog-").tempdir_in("/tmp").unwrap();
    let socket=dir.path().join("daemon.sock").to_string_lossy().into_owned();
    let script=r#"import os,sys,time,tty
tty.setraw(0)
root=sys.argv[1]
os.write(1,b'HOST_READY\r\n')
while not os.path.exists(root+'/burst'):time.sleep(.005)
data=b'x'*(3*1024*1024)+b'\r\nBACKLOG_END\r\n'
while data:
    n=os.write(1,data);data=data[n:]
open(root+'/burst-done','w').write('done')
count=0
while count<2*1024*1024:count+=len(os.read(0,min(65536,2*1024*1024-count)))
open(root+'/input-done','w').write(str(os.getpid())+':'+str(count))
os.write(1,b'INPUT_DELIVERED\r\n')
while True:time.sleep(.1)
"#;
    let spec=SpawnSpec {program:Some("/usr/bin/python3".into()),args:vec!["-c".into(),script.into(),dir.path().to_string_lossy().into_owned()],cwd:Some(dir.path().to_string_lossy().into_owned()),cols:120,rows:32,..Default::default()};
    let owned=OwnedHost(HostPty::spawn(&spec,77,std::path::Path::new(env!("CARGO_BIN_EXE_kiln")),&socket).unwrap());
    let original_pid=owned.0.pid();
    let mut initial=owned.0.reader().unwrap();let mut bytes=[0;1024];
    let deadline=Instant::now()+Duration::from_secs(5);
    loop {
        if let kiln_daemon::pty::ReadResult::Data(n)=initial.read_timeout(&mut bytes,100).unwrap(){if String::from_utf8_lossy(&bytes[..n]).contains("HOST_READY"){break;}}
        assert!(Instant::now()<deadline,"owned child never became ready");
    }
    drop(initial);
    let relay=HostRelay::install(&owned.0.endpoint);
    kiln_daemon::ptyhost::save_registry(&socket,&[HostRecord{session:77,endpoint:owned.0.endpoint.clone(),name:Some("backlog fixture".into()),workspace:None,cwd:spec.cwd.clone(),created_unix:0}]);
    let (fixture,client)=Fixture::start_in(dir,true);
    wait_text(&client,77,"HOST_READY");
    let other=spawn(&client,"stty -echo; printf 'OTHER_READY\\r\\n'; read line; printf 'OTHER_ECHO=%s\\r\\n' \"$line\"; read hold");
    wait_text(&client,other,"OTHER_READY");
    if reconnect {relay.interrupt_and_hold();}
    else {relay.pause_input.store(true,std::sync::atomic::Ordering::SeqCst);}
    // >512KiB host decoder capacity, and >local socket capacity input. Calling
    // resize before starting the replacement consumer forms a real I/O cycle.
    client.send(ClientMsg::Input{session:77,data:vec![b'a';2*1024*1024]});
    let resize_control=if reconnect {None} else {
        let deadline=Instant::now()+Duration::from_secs(3);
        while !relay.input_blocked.load(std::sync::atomic::Ordering::SeqCst){
            assert!(Instant::now()<deadline,"large input never reached the held real relay");std::thread::sleep(Duration::from_millis(5));
        }
        let control=Client::connect(&fixture.socket,None).unwrap();
        control.send(ClientMsg::Resize{session:77,cols:121,rows:32});
        // This connection remains independent of the handler writing Resize.
        // A held input frame must not reserve the emulator while control waits.
        let observer=Client::connect(&fixture.socket,None).unwrap();
        assert!(matches!(observer.request(|req|ClientMsg::ReadText{req,session:77,history:0},Duration::from_secs(2)).unwrap(),ServerMsg::Text{..}));
        Some(control)
    };
    std::fs::write(fixture._dir.path().join("burst"),b"go").unwrap();
    let deadline=Instant::now()+Duration::from_secs(5);
    while !fixture._dir.path().join("burst-done").exists(){
        assert!(Instant::now()<deadline,"detached host did not buffer real output");std::thread::sleep(Duration::from_millis(5));
    }
    relay.pause_input.store(false,std::sync::atomic::Ordering::SeqCst);
    relay.release();
    client.send(ClientMsg::Input{session:other,data:b"responsive\n".to_vec()});
    assert!(matches!(client.request(|req|ClientMsg::Ping{req},Duration::from_secs(2)).unwrap(),ServerMsg::Pong{..}));
    wait_text(&client,other,"OTHER_ECHO=responsive");
    let expected=format!("{original_pid}:{}",2*1024*1024);
    let deadline=Instant::now()+Duration::from_secs(12);
    loop {
        if std::fs::read_to_string(fixture._dir.path().join("input-done")).ok().as_deref()==Some(expected.as_str()){break;}
        assert!(Instant::now()<deadline,"repaired same child never received queued input");std::thread::sleep(Duration::from_millis(10));
    }
    wait_text(&client,77,"INPUT_DELIVERED");
    let ServerMsg::Sessions{sessions,..}=client.request(|req|ClientMsg::ListSessions{req},Duration::from_secs(2)).unwrap() else{panic!("expected sessions");};
    let repaired=sessions.iter().find(|s|s.id==77).unwrap();assert_eq!(repaired.pid,original_pid);assert!(repaired.exited.is_none());
    client.send(ClientMsg::Kill{session:77});client.send(ClientMsg::Kill{session:other});
    drop(resize_control);drop(client);drop(fixture);drop(relay);drop(owned);
}

#[test]
fn hosted_reconnect_drains_large_backlog_before_queued_input_and_redraw_control() {hosted_backpressure_case(true);}

#[test]
fn hosted_resize_never_holds_emulator_while_input_transport_is_backpressured() {hosted_backpressure_case(false);}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Only this fixture's private socket and owned child are touched.
        if let Ok(client) = Client::connect(&self.socket, None) { client.send(ClientMsg::Shutdown); }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() { return; }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn(client: &Client, script: &str) -> u64 {
    let response = client.request(|req| ClientMsg::Create { req, spec: SpawnSpec {
        program: Some("/bin/sh".into()), args: vec!["-c".into(), script.into()], cols: 100, rows: 12,
        ..Default::default()
    } }, Duration::from_secs(10)).unwrap();
    let ServerMsg::Created { session, .. } = response else { panic!("unexpected {response:?}") };
    client.send(ClientMsg::Attach { session, cols: 100, rows: 12 });
    session
}

fn wait_frame(client: &Client, session: u64, text: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let msg = client.rx.recv_timeout(left).unwrap_or_else(|e| panic!("frame never recovered: {text}: {e}"));
        if let ServerMsg::Frame(frame) = msg {
            if frame.session == session && frame.lines.iter().any(|(_, line)| line.text().contains(text)) { return; }
        }
    }
}

fn wait_text(client: &Client, session: u64, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let response = client.request(|req| ClientMsg::ReadText { req, session, history: 100 }, Duration::from_secs(1)).unwrap();
        if let ServerMsg::Text { text, .. } = response {
            if text.contains(expected) { return; }
        }
        assert!(Instant::now() < deadline, "terminal never recovered: {expected}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn incomplete_tui_redraw_recovers_input_echo_frames_and_reattachment_for_both_pty_modes() {
    for hosted in [false, true] {
        let (fixture, client) = Fixture::start(hosted);
        // Deliberately omit ESU both before and after reading input. The child
        // remains alive, as it does in the reported single-panel freeze.
        let session = spawn(&client, "stty -echo; printf 'READY\\r\\n\\033[?2026hWORKING'; read line; printf '\\r\\n\\033[?2026hECHO=%s' \"$line\"; read line; printf '\\r\\nBURST\\r\\n'; i=0; while [ \"$i\" -lt 3000 ]; do printf 'line %s\\r\\n' \"$i\"; i=$((i+1)); done; printf 'BURST_DONE\\r\\n'; read hold");
        wait_frame(&client, session, "WORKING");
        client.send(ClientMsg::Input { session, data: "한?\n".as_bytes().to_vec() });
        wait_frame(&client, session, "ECHO=한?");
        wait_text(&client, session, "ECHO=한?");

        // A client that stops consuming frames must not stop this PTY or an
        // independent session. Its private socket is dropped after the burst.
        let mut slow = UnixStream::connect(&fixture.socket).unwrap();
        slow.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        write_msg(&mut slow, &ClientMsg::Hello { proto: kiln_proto::PROTO_VERSION, build: "fixture".into(), client: "slow-fixture".into() }).unwrap();
        assert!(matches!(read_msg::<_, ServerMsg>(&mut slow).unwrap(), Some(ServerMsg::Hello { .. })));
        write_msg(&mut slow, &ClientMsg::Attach { session, cols: 100, rows: 12 }).unwrap();

        let other = spawn(&client, "stty -echo; printf 'OTHER_READY\\r\\n'; read line; printf 'OTHER_ECHO=%s\\r\\n' \"$line\"; read hold");
        wait_frame(&client, other, "OTHER_READY");
        client.send(ClientMsg::Input { session, data: b"burst\n".to_vec() });
        client.send(ClientMsg::Input { session: other, data: b"alive\n".to_vec() });
        wait_text(&client, other, "OTHER_ECHO=alive");
        wait_text(&client, session, "BURST_DONE");
        drop(slow);

        client.send(ClientMsg::Detach { session });
        client.send(ClientMsg::Attach { session, cols: 100, rows: 12 });
        wait_frame(&client, session, "BURST_DONE");
        drop(client);
        let replacement = Client::connect(&fixture.socket, None).unwrap();
        replacement.send(ClientMsg::Attach { session, cols: 100, rows: 12 });
        wait_frame(&replacement, session, "BURST_DONE");
        let response = replacement.request(|req| ClientMsg::ListSessions { req }, Duration::from_secs(1)).unwrap();
        let ServerMsg::Sessions { sessions, .. } = response else { panic!("unexpected response") };
        assert!(sessions.iter().any(|info| info.id == session && info.exited.is_none()));
        replacement.send(ClientMsg::Kill { session });
        replacement.send(ClientMsg::Kill { session: other });
    }
}

#[test]
fn manual_repair_requests_real_redraw_without_replacing_the_running_child() {
    for hosted in [false,true] {
        let (_fixture,client)=Fixture::start(hosted);
        let script=r#"import os,signal,time
signal.signal(signal.SIGWINCH,lambda *_:os.write(1,('REDRAW:'+str(os.getpid())+'\r\n').encode()))
os.write(1,b'REPAIR_READY\r\n')
while True:time.sleep(.05)
"#;
        let ServerMsg::Created{session,..}=client.request(|req|ClientMsg::Create{req,spec:SpawnSpec{program:Some("/usr/bin/python3".into()),args:vec!["-c".into(),script.into()],cols:100,rows:12,..Default::default()}},Duration::from_secs(5)).unwrap() else{panic!("expected owned child");};
        client.send(ClientMsg::Attach{session,cols:100,rows:12});wait_frame(&client,session,"REPAIR_READY");
        let ServerMsg::Sessions{sessions,..}=client.request(|req|ClientMsg::ListSessions{req},Duration::from_secs(2)).unwrap() else{panic!("expected sessions");};
        let pid=sessions.iter().find(|s|s.id==session).unwrap().pid;
        client.send(ClientMsg::RecoverTerminal{session});wait_text(&client,session,&format!("REDRAW:{pid}"));
        let ServerMsg::Sessions{sessions,..}=client.request(|req|ClientMsg::ListSessions{req},Duration::from_secs(2)).unwrap() else{panic!("expected sessions");};
        let same=sessions.iter().find(|s|s.id==session).unwrap();assert_eq!(same.pid,pid);assert!(same.exited.is_none());assert_eq!((same.cols,same.rows),(100,12));
        client.send(ClientMsg::Kill{session});
    }
}

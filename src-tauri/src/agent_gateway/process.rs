//! Per-adapter lifecycle owner. macOS uses kernel lineage and audit-token signals.
use std::ops::{Deref, DerefMut};
use tokio::process::{Child, Command};

pub(crate) struct OwnedChild {
    child: Child,
    #[cfg(unix)]
    lifetime: Option<std::os::unix::net::UnixStream>,
}
impl Deref for OwnedChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.child
    }
}
impl DerefMut for OwnedChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}
impl OwnedChild {
    pub(crate) fn start_kill(&mut self) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            self.lifetime.take();
            Ok(())
        }
        #[cfg(not(unix))]
        {
            self.child.start_kill()
        }
    }
    pub(crate) async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let status = self.child.wait().await?;
        #[cfg(target_os = "macos")]
        if !status.success() {
            return Err(std::io::Error::other(
                "process owner could not verify complete cleanup; see stderr",
            ));
        }
        Ok(status)
    }
    pub(crate) async fn kill(&mut self) -> std::io::Result<()> {
        self.start_kill()?;
        self.wait().await.map(|_| ())
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            self.lifetime.take();
        }
    }
}
pub(crate) fn spawn(command: &mut Command) -> std::io::Result<OwnedChild> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let (guard, receiver) = std::os::unix::net::UnixStream::pair()?;
        let fd = receiver.as_raw_fd();
        let original = command.as_std();
        #[cfg(target_os = "macos")]
        let mut supervisor = {
            let executable = std::env::var_os("AGENTGATE_PROCESS_OWNER_BIN")
                .map(std::path::PathBuf::from)
                .unwrap_or(std::env::current_exe()?);
            let mut owner = Command::new(executable);
            #[cfg(not(test))]
            owner
                .arg("--agentgate-process-owner")
                .arg(original.get_program())
                .args(original.get_args());
            #[cfg(test)]
            {
                let args: Vec<_> = std::iter::once(original.get_program())
                    .chain(original.get_args())
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect();
                owner
                    .args([
                        "--exact",
                        "agent_gateway::process::tests::native_owner_helper",
                        "--ignored",
                        "--nocapture",
                    ])
                    .env("AGENTGATE_NATIVE_OWNER_ARGS", serde_json::to_string(&args)?);
            }
            owner
        };
        #[cfg(not(target_os = "macos"))]
        let mut supervisor = Command::new("/bin/sh");
        // 独立进程组归 supervisor 所有，不按磁盘记录或猜测 PID 杀进程。
        // 子 CLI 退出和私有寿命管道 EOF 都触发清理；watchdog 不持有 CLI stdout。
        #[cfg(not(target_os = "macos"))]
        supervisor
            .arg("-c")
            .arg(
                r#"
"$@" <&0 3<&- &
agent=$!
(
    trap '' TERM
    IFS= read -r signal <&3
    kill -TERM 0
    sleep 2
    kill -KILL 0
) </dev/null >/dev/null 2>&1 &
exec 0<&- 3<&- 1>&- 2>&-
wait "$agent"
kill -KILL 0
"#,
            )
            .arg("agentgate-supervisor")
            .arg(original.get_program())
            .args(original.get_args());
        for (key, value) in original.get_envs() {
            if let Some(value) = value {
                supervisor.env(key, value);
            } else {
                supervisor.env_remove(key);
            }
        }
        if let Some(cwd) = original.get_current_dir() {
            supervisor.current_dir(cwd);
        }
        supervisor
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(false)
            .process_group(0);
        #[cfg(target_os = "macos")]
        supervisor.stderr(std::process::Stdio::inherit());
        #[cfg(all(not(target_os = "macos"), test))]
        supervisor.stderr(std::process::Stdio::inherit());
        #[cfg(all(not(target_os = "macos"), not(test)))]
        supervisor.stderr(std::process::Stdio::null());
        unsafe {
            supervisor.pre_exec(move || {
                if libc::dup2(fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::fcntl(3, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                #[cfg(all(test, target_os = "macos"))]
                {
                    if libc::dup2(1, 4) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    let null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
                    if null == -1 || libc::dup2(null, 1) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if null != 4 {
                        libc::close(null);
                    }
                    libc::fcntl(4, libc::F_SETFD, 0);
                }
                Ok(())
            });
        }
        let child = supervisor.spawn()?;
        #[cfg(target_os = "macos")]
        let mut child = child;
        drop(receiver);
        #[cfg(target_os = "macos")]
        {
            use std::io::Read;
            guard.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
            let mut ready = [0];
            if let Err(error) = (&guard).read_exact(&mut ready) {
                let _ = child.start_kill();
                return Err(std::io::Error::other(format!(
                    "process owner initialization failed: {error}"
                )));
            }
            if ready != [b'R'] {
                return Err(std::io::Error::other("invalid process owner handshake"));
            }
            guard.set_read_timeout(None)?;
        }
        Ok(OwnedChild {
            child,
            lifetime: Some(guard),
        })
    }
    #[cfg(not(unix))]
    {
        Ok(OwnedChild {
            child: command.spawn()?,
        })
    }
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) fn failed_owner_for_test() -> OwnedChild {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "exit 125"]);
    OwnedChild {
        child: command.spawn().unwrap(),
        #[cfg(unix)]
        lifetime: None,
    }
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) fn pending_owner_for_test() -> OwnedChild {
    let mut command = Command::new("/bin/sleep");
    command.arg("120").kill_on_drop(true);
    OwnedChild {
        child: command.spawn().unwrap(),
        lifetime: None,
    }
}

/// Dispatch before starting runtimes or parsing user commands. This process owns
/// exactly one adapter and exits when its provider or lifetime FD ends.
pub fn dispatch_process_owner() {
    #[cfg(target_os = "macos")]
    if std::env::args_os().nth(1).as_deref()
        == Some(std::ffi::OsStr::new("--agentgate-process-owner"))
    {
        use std::os::unix::ffi::OsStrExt;
        let args = std::env::args_os()
            .skip(2)
            .map(|s| std::ffi::CString::new(s.as_bytes()).expect("argv contains NUL"))
            .collect::<Vec<_>>();
        run_native_owner(args);
    }
}
#[cfg(target_os = "macos")]
fn run_native_owner(args: Vec<std::ffi::CString>) -> ! {
    unsafe extern "C" {
        fn agentgate_process_owner(argc: libc::c_int, argv: *mut *mut libc::c_char) -> libc::c_int;
    }
    let mut pointers = args
        .iter()
        .map(|s| s.as_ptr().cast_mut())
        .collect::<Vec<_>>();
    pointers.push(std::ptr::null_mut());
    std::process::exit(unsafe { agentgate_process_owner(args.len() as _, pointers.as_mut_ptr()) });
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "native process owner subprocess entry"]
    fn native_owner_helper() {
        unsafe {
            assert_ne!(libc::dup2(4, 1), -1);
            libc::close(4);
        }
        let args: Vec<String> =
            serde_json::from_str(&std::env::var("AGENTGATE_NATIVE_OWNER_ARGS").unwrap()).unwrap();
        run_native_owner(
            args.into_iter()
                .map(|s| std::ffi::CString::new(s).unwrap())
                .collect(),
        );
    }
    fn alive(pid: &str) -> bool {
        std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }
    #[tokio::test]
    #[ignore = "subprocess helper"]
    async fn owner_helper() {
        let path = std::env::var("AGENT_GATE_OWNER_TEST").unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("echo $$ > \"$1\"; sleep 120 & echo $! >> \"$1\"; wait")
            .arg("probe")
            .arg(path);
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let _child = spawn(&mut command).unwrap();
        std::future::pending::<()>().await;
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn owner_ready_confirms_root_group_isolation_before_immediate_close() {
        futures::future::join_all((0..16).map(|_| async {
            let mut command = Command::new("/bin/sleep");
            command.arg("120");
            let mut owned = spawn(&mut command).unwrap();
            // /bin/sleep is gated until readiness; find the unique direct child.
            let owner_pid = owned.id().unwrap();
            let output = std::process::Command::new("/bin/ps")
                .args(["-axo", "pid=,ppid=,pgid="])
                .output()
                .unwrap();
            let root = String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .find_map(|line| {
                    let fields: Vec<u32> = line
                        .split_whitespace()
                        .filter_map(|s| s.parse().ok())
                        .collect();
                    if fields.len() == 3 && fields[1] == owner_pid {
                        Some((fields[0], fields[2]))
                    } else {
                        None
                    }
                })
                .expect("ready owner must own its direct provider child");
            assert_eq!(
                root.0, root.1,
                "root must have an isolated group at readiness"
            );
            assert_ne!(root.1, owner_pid, "root cannot share owner group");
            tokio::time::timeout(Duration::from_secs(5), owned.kill())
                .await
                .unwrap()
                .unwrap();
        }))
        .await;
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn provider_group_shutdown_cannot_kill_its_owner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("detached");
        let mut command = Command::new("/usr/bin/python3");
        command.args(["-c", "import os,time,sys,signal; p=os.fork(); os.setsid() if p==0 else None; open(sys.argv[1]+('child' if p==0 else 'root'),'w').write(str(os.getpid())); time.sleep(120) if p==0 else sys.stdin.read(); os.killpg(os.getpgrp(),signal.SIGKILL)"]).arg(&path);
        let mut owned = spawn(&mut command).unwrap();
        let child_path = path.with_file_name("detachedchild");
        let root_path = path.with_file_name("detachedroot");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !root_path.exists() || !child_path.exists() {
            assert!(std::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let child = std::fs::read_to_string(child_path).unwrap();
        owned.stdin.take();
        let result = tokio::time::timeout(Duration::from_secs(5), owned.wait()).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let survivor = alive(&child);
        if survivor {
            std::process::Command::new("kill")
                .args(["-KILL", &child])
                .status()
                .unwrap();
        }
        assert!(
            result.unwrap().is_ok(),
            "provider group shutdown killed lifecycle owner"
        );
        assert!(
            !survivor,
            "provider group shutdown left detached descendant"
        );
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn root_sigkill_reaps_detached_session_without_harming_unrelated_process() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("detached");
        let mut unrelated = std::process::Command::new("/bin/sleep")
            .arg("120")
            .spawn()
            .unwrap();
        let mut command = Command::new("/usr/bin/python3");
        command.args(["-c", "import os,time,sys; p=os.fork(); os.setsid() if p==0 else None; open(sys.argv[1]+('child' if p==0 else 'root'),'w').write(str(os.getpid())); time.sleep(120)"]).arg(&path);
        let mut owned = spawn(&mut command).unwrap();
        let parallel_path = dir.path().join("parallel");
        let mut parallel_command = Command::new("/usr/bin/python3");
        parallel_command.args(["-c", "import os,time,sys; open(sys.argv[1],'w').write(str(os.getpid())); time.sleep(120)"]).arg(&parallel_path);
        let mut parallel = spawn(&mut parallel_command).unwrap();
        let root_path = path.with_file_name("detachedroot");
        let child_path = path.with_file_name("detachedchild");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !root_path.exists() || !child_path.exists() || !parallel_path.exists() {
            assert!(std::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let root = std::fs::read_to_string(root_path).unwrap();
        let child = std::fs::read_to_string(child_path).unwrap();
        assert!(alive(&root) && alive(&child));
        std::process::Command::new("kill")
            .args(["-KILL", &root])
            .status()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), owned.wait())
            .await
            .unwrap()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let survivor = alive(&child);
        let parallel_pid = std::fs::read_to_string(parallel_path).unwrap();
        let parallel_alive = alive(&parallel_pid);
        parallel.kill().await.unwrap();
        assert!(
            !alive(&parallel_pid),
            "normal owner cancellation must reap its root"
        );
        let unrelated_alive = unrelated.try_wait().unwrap().is_none();
        if survivor {
            std::process::Command::new("kill")
                .args(["-KILL", &child])
                .status()
                .unwrap();
        }
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
        assert!(unrelated_alive, "unrelated process must survive");
        assert!(parallel_alive, "concurrent independent owner must survive");
        assert!(!survivor, "setsid descendant survived root SIGKILL");
    }
    #[tokio::test]
    async fn service_sigkill_reaps_cli_and_its_child() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pids");
        let mut owner = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "agent_gateway::process::tests::owner_helper",
                "--ignored",
            ])
            .env("AGENT_GATE_OWNER_TEST", &path)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let pids = loop {
            let raw = std::fs::read_to_string(&path).unwrap_or_default();
            let pids: Vec<String> = raw.lines().map(str::to_owned).collect();
            if pids.len() == 2 {
                break pids;
            }
            assert!(std::time::Instant::now() < deadline, "probe not ready");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert!(
            pids.iter().all(|pid| alive(pid)),
            "probe processes must be alive before owner SIGKILL"
        );
        owner.kill().unwrap();
        owner.wait().unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let survivors: Vec<_> = pids.iter().filter(|pid| alive(pid)).collect();
        for pid in &survivors {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", pid])
                .status();
        }
        assert!(
            survivors.is_empty(),
            "service SIGKILL left owned processes alive: {survivors:?}"
        );
    }
}

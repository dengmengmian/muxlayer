//! Unix guardian 仅拥有专属进程组；服务死亡不能依赖 Rust Drop 执行。
//! CLI 自行 setsid 的后代不在该组，Windows 当前仍只有单进程 Child 控制。
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
    pub(crate) async fn kill(&mut self) -> std::io::Result<()> {
        self.start_kill()?;
        self.child.wait().await.map(|_| ())
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
        let mut supervisor = Command::new("/bin/sh");
        // 独立进程组归 supervisor 所有，不按磁盘记录或猜测 PID 杀进程。
        // 子 CLI 退出和私有寿命管道 EOF 都触发清理；watchdog 不持有 CLI stdout。
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
            .stderr(std::process::Stdio::null())
            .kill_on_drop(false)
            .process_group(0);
        unsafe {
            supervisor.pre_exec(move || {
                if libc::dup2(fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::fcntl(3, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = supervisor.spawn()?;
        drop(receiver);
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;
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

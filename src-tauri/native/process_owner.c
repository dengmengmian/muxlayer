// macOS per-adapter owner. No daemon, polling watcher, PID-only signalling or killpg.
#include <errno.h>
#include <fcntl.h>
#include <libproc.h>
#include <mach/mach.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/event.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

// XNU PROC_PIDUNIQIDENTIFIERINFO. Fail closed if the kernel ABI differs.
struct identity { uint8_t uuid[16]; uint64_t unique, parent; int32_t version, original_parent_version; uint64_t reserved[2]; };
_Static_assert(sizeof(struct identity) == 56, "kernel identity ABI");
struct member { pid_t pid; uint64_t unique; };
static struct member *members;
static size_t count, capacity;
static struct timespec cleanup_deadline;
static int deadline_elapsed(void) {
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    return now.tv_sec > cleanup_deadline.tv_sec || (now.tv_sec == cleanup_deadline.tv_sec && now.tv_nsec >= cleanup_deadline.tv_nsec);
}
static int identity(pid_t pid, struct identity *out) {
    int size = proc_pidinfo(pid, 17, 0, out, sizeof *out);
    if (size == sizeof *out && out->unique) return 1;
    if (size == 0 && (errno == ESRCH || errno == ENOENT)) return 0;
    fprintf(stderr, "agentgate owner: kernel identity unavailable pid=%d bytes=%d errno=%d\n", pid, size, errno);
    return -1;
}
static int known(uint64_t unique) {
    for (size_t i = 0; i < count; i++) if (members[i].unique == unique) return 1;
    return 0;
}
static int remember(pid_t pid, uint64_t unique) {
    if (known(unique)) return 0;
    if (count == capacity) {
        size_t next = capacity ? capacity * 2 : 32;
        void *p = realloc(members, next * sizeof *members);
        if (!p) return -1;
        members = p; capacity = next;
    }
    members[count++] = (struct member){pid, unique};
    return 1;
}
static int signal_member(struct member member, int sig) {
    struct identity before = {0};
    int exists = identity(member.pid, &before);
    if (exists <= 0) return exists;
    if (before.unique != member.unique) return 0;
    struct proc_bsdinfo bsd = {0};
    if (proc_pidinfo(member.pid, PROC_PIDTBSDINFO, 0, &bsd, sizeof bsd) == sizeof bsd && bsd.pbi_status == 5) return 0;
    mach_port_name_t task = MACH_PORT_NULL;
    kern_return_t kr = task_name_for_pid(mach_task_self(), member.pid, &task);
    audit_token_t token = {0};
    mach_msg_type_number_t n = TASK_AUDIT_TOKEN_COUNT;
    if (kr == KERN_SUCCESS) kr = task_info(task, TASK_AUDIT_TOKEN, (task_info_t)&token, &n);
    if (task != MACH_PORT_NULL) mach_port_deallocate(mach_task_self(), task);
    struct identity after = {0};
    exists = identity(member.pid, &after);
    if (exists <= 0) return exists;
    if (after.unique != member.unique) return 0;
    // The token pins pidversion atomically at delivery, including an exec race.
    if (kr != KERN_SUCCESS || n != TASK_AUDIT_TOKEN_COUNT || token.val[5] != (uint32_t)member.pid || token.val[7] != (uint32_t)after.version) {
        fprintf(stderr, "agentgate owner: audit identity unavailable pid=%d kr=%d\n", member.pid, kr);
        return -1;
    }
    int rc = proc_signal_with_audittoken(&token, sig);
    if (rc == 0 || rc == ESRCH || (rc == -1 && errno == ESRCH)) return 0;
    fprintf(stderr, "agentgate owner: identity-bound signal failed pid=%d signal=%d rc=%d errno=%d\n", member.pid, sig, rc, errno);
    return -1;
}
static int discover(void) {
    int slots = proc_listallpids(NULL, 0);
    if (slots <= 0) return -1;
    slots += 128;
    pid_t *pids = calloc((size_t)slots, sizeof *pids);
    if (!pids) return -1;
    int n = proc_listallpids(pids, slots * (int)sizeof *pids), added = 0;
    if (n <= 0 || n >= slots) { free(pids); return -1; }
    // An exited ancestor can remain in parent_unique until orphan exec. Preserve
    // every proven member's own unique identity across subsequent reparent/exec.
    for (int pass = 0; pass < 32; pass++) {
        int changed = 0;
        for (int i = 0; i < n; i++) {
            struct identity candidate = {0};
            int size = proc_pidinfo(pids[i], 17, 0, &candidate, sizeof candidate);
            if (size != sizeof candidate || !candidate.unique || !known(candidate.parent) || known(candidate.unique)) continue;
            int rc = remember(pids[i], candidate.unique);
            if (rc < 0 || signal_member(members[count - 1], SIGSTOP) < 0) { free(pids); return -1; }
            changed += rc; added += rc;
        }
        if (!changed) { free(pids); return added; }
    }
    free(pids); return -1;
}
// Confirm kernel stop/exit state before treating a scan as stable. These waits
// occur only during the bounded exit transaction, not during provider execution.
static int await_members(int stopped) {
    for (int attempt = 0; attempt < 200; attempt++) {
        int pending = 0;
        for (size_t i = 0; i < count; i++) {
            struct identity current = {0};
            int exists = identity(members[i].pid, &current);
            if (exists < 0) return -1;
            if (!exists || current.unique != members[i].unique) continue;
            struct proc_bsdinfo bsd = {0};
            if (proc_pidinfo(members[i].pid, PROC_PIDTBSDINFO, 0, &bsd, sizeof bsd) != sizeof bsd) { pending++; continue; }
            if (bsd.pbi_status == 5 || (stopped && bsd.pbi_status == 4)) continue;
            if (stopped && signal_member(members[i], SIGSTOP) < 0) return -1;
            if (attempt == 199 || deadline_elapsed()) fprintf(stderr, "agentgate owner: pending pid=%d unique=%llu state=%u stopped=%d\n", members[i].pid, members[i].unique, bsd.pbi_status, stopped);
            pending++;
        }
        if (!pending) return 0;
        if (deadline_elapsed()) break;
        usleep(10000);
    }
    fprintf(stderr, "agentgate owner: timed out awaiting owned process %s\n", stopped ? "stop" : "exit");
    return -1;
}
static int cleanup(void) {
    clock_gettime(CLOCK_MONOTONIC, &cleanup_deadline);
    cleanup_deadline.tv_sec += 3;
    int failed = signal_member(members[0], SIGSTOP) < 0;
    // Only lifecycle-triggered bounded drain, never periodic process scanning.
    int stable = 0;
    for (int pass = 0; pass < 32 && stable < 2 && !deadline_elapsed(); pass++) {
        if (await_members(1) < 0) { failed = 1; break; }
        int added = discover();
        if (added < 0) { failed = 1; break; }
        stable = added ? 0 : stable + 1;
    }
    if (stable < 2) failed = 1;
    fprintf(stderr, "agentgate owner: cleanup root_unique=%llu members=%zu\n", members[0].unique, count);
    for (size_t i = count; i > 0; i--) if (signal_member(members[i - 1], SIGKILL) < 0) failed = 1;
    if (await_members(0) < 0) failed = 1;
    return failed ? -1 : 0;
}
int agentgate_process_owner(int argc, char **argv) {
    if (argc < 1) return 125;
    int gate[2];
    if (pipe(gate)) return 125;
    pid_t root = fork();
    if (root == -1) return 125;
    if (!root) {
        close(gate[1]); close(3);
        // The provider may kill its own process group during stdin-EOF teardown.
        // Keep the owner outside that group; groups are never ownership evidence.
        if (setpgid(0, 0) == -1) _exit(125);
        char allow;
        if (read(gate[0], &allow, 1) != 1) _exit(125);
        close(gate[0]);
        // Preserve previous adapter stderr policy; owner diagnostics remain visible.
        int null = open("/dev/null", O_WRONLY);
        if (null == -1 || dup2(null, 2) == -1) _exit(125);
        if (null != 2) close(null);
        execvp(argv[0], argv);
        perror("agentgate owner: exec"); _exit(127);
    }
    close(gate[0]);
    struct identity root_identity = {0};
    int kq = kqueue();
    struct kevent changes[2], event;
    EV_SET(&changes[0], root, EVFILT_PROC, EV_ADD, NOTE_EXIT, 0, NULL);
    EV_SET(&changes[1], 3, EVFILT_READ, EV_ADD, 0, 0, NULL);
    // Parent establishes isolation synchronously: readiness cannot race an
    // unscheduled child that has not reached its own setpgid call yet.
    int isolated = setpgid(root, root) == 0 && getpgid(root) == root;
    if (!isolated || identity(root, &root_identity) != 1 || remember(root, root_identity.unique) < 0 || kq == -1 || kevent(kq, changes, 2, NULL, 0, NULL) == -1) {
        // Root is still our blocked, unreaped direct child: its PID cannot reuse.
        close(gate[1]); kill(root, SIGKILL); waitpid(root, NULL, 0);
        fprintf(stderr, "agentgate owner: initialization failed closed\n"); return 125;
    }
    fprintf(stderr, "agentgate owner: root pid=%d unique=%llu\n", root, root_identity.unique);
    if (write(3, "R", 1) != 1 || write(gate[1], "R", 1) != 1) {
        close(gate[1]); int rc = cleanup(); waitpid(root, NULL, rc ? WNOHANG : 0); return rc ? 125 : 0;
    }
    close(gate[1]); close(0); close(1);
    int rc;
    do { rc = kevent(kq, NULL, 0, &event, 1, NULL); } while (rc == -1 && errno == EINTR);
    int clean = cleanup();
    int failed = rc != 1 || clean < 0;
    int status = 0;
    if (failed) { (void)waitpid(root, &status, WNOHANG); }
    else while (waitpid(root, &status, 0) == -1) if (errno != EINTR) { failed = 1; break; }
    close(kq); close(3); free(members);
    // 125 means ownership/cleanup failure, distinct from provider exit status.
    if (failed) { fprintf(stderr, "agentgate owner: cleanup incomplete\n"); return 125; }
    return 0;
}

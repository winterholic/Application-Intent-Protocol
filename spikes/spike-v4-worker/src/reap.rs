//! Tears down a worker and everything it started.
//!
//! `Child::kill` only signals the direct child. An extension can `fork`, `setsid` and outlive it,
//! so the worker is started as its own process-group leader and the whole tree is killed:
//! first the descendants found through the parent-pid links (this also reaches children that
//! left the group with `setsid`), then the group. Linux hosts that configure a cgroup get a
//! stronger guarantee from `cgroup.kill`; this covers macOS and cgroup-less Linux.
//!
//! Limit: a descendant that was already orphaned (double-fork daemon re-parented to init) before
//! teardown starts has no parent link left to follow. The macOS sandbox denies `process-fork`,
//! and Bubblewrap runs in its own PID namespace, so those modes cannot create one.

#[cfg(unix)]
mod imp {
    use std::collections::{HashMap, HashSet};

    const MAX_ROUNDS: usize = 10;

    fn process_table() -> HashMap<i32, i32> {
        let Ok(output) = std::process::Command::new("ps").args(["-A", "-o", "pid=,ppid=,stat="]).output() else {
            return HashMap::new();
        };
        let mut parent_of = HashMap::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let mut fields = line.split_whitespace();
            let (pid, ppid, stat) = (fields.next().map(str::parse::<i32>), fields.next().map(str::parse::<i32>), fields.next());
            // A zombie is already dead and only waits for its parent to reap it.
            if let (Some(Ok(pid)), Some(Ok(ppid)), Some(stat)) = (pid, ppid, stat) {
                if !stat.starts_with('Z') {
                    parent_of.insert(pid, ppid);
                }
            }
        }
        parent_of
    }

    fn descendants(root: i32, parent_of: &HashMap<i32, i32>) -> Vec<i32> {
        let mut found = Vec::new();
        let mut seen: HashSet<i32> = HashSet::from([root]);
        let mut frontier = vec![root];
        while let Some(parent) = frontier.pop() {
            for (&pid, &ppid) in parent_of {
                if ppid == parent && seen.insert(pid) {
                    found.push(pid);
                    frontier.push(pid);
                }
            }
        }
        found
    }

    /// Kills `root`, its process group, and every descendant reachable through parent links.
    /// `root` must be a process-group leader created with `process_group(0)`.
    pub(crate) fn kill_tree(root: u32) {
        let Ok(root) = i32::try_from(root) else { return };
        if root <= 1 {
            return;
        }
        for _ in 0..MAX_ROUNDS {
            let table = process_table();
            let mut victims = descendants(root, &table);
            victims.push(root);
            // Freeze first so a process cannot fork between the scan and the kill.
            for pid in &victims {
                unsafe { libc::kill(*pid, libc::SIGSTOP) };
            }
            let late = descendants(root, &process_table());
            for pid in late.iter().filter(|pid| !victims.contains(pid)) {
                unsafe { libc::kill(*pid, libc::SIGSTOP) };
            }
            unsafe { libc::killpg(root, libc::SIGKILL) };
            for pid in victims.iter().chain(late.iter()) {
                unsafe { libc::kill(*pid, libc::SIGKILL) };
            }
            if descendants(root, &process_table()).is_empty() {
                break;
            }
        }
    }
}

#[cfg(unix)]
pub(crate) use imp::kill_tree;

#[cfg(not(unix))]
pub(crate) fn kill_tree(_root: u32) {}

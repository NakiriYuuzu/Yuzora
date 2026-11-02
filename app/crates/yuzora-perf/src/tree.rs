//! Which processes count and in which category, over rows from the process
//! table. Pure, so the tests need no real processes.
//!
//! - **app**: the app's own process.
//! - **core**: herdr-core's server and holders (`hc server status`) and
//!   everything under them: the terminals' programs and agents.
//! - **tools**: everything else under the app (git, ssh, database helpers,
//!   `hc connect`, a mod's `exec`). What can't be told apart lands here too.
//!
//! Each pid is counted once, in one category, so the categories always add up
//! to the total (memory, CPU and process count).

use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};

/// One process, as the table had it at the last refresh.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub pid: u32,
    pub parent: Option<u32>,
    /// Relative to one core: 100 is one core busy, and it can go past 100.
    pub cpu: f32,
    /// RSS, bytes.
    pub memory: u64,
}

/// What a set of processes uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Usage {
    /// The arithmetic sum of each process's CPU (one core = 100): not divided
    /// by the number of cores, as in beta.1.
    pub cpu: f32,
    pub memory: u64,
    pub count: u32,
}

impl Usage {
    fn add(&mut self, row: &Row) {
        self.cpu += row.cpu;
        self.memory = self.memory.saturating_add(row.memory);
        self.count = self.count.saturating_add(1);
    }

    fn plus(self, other: Usage) -> Usage {
        Usage {
            cpu: self.cpu + other.cpu,
            memory: self.memory.saturating_add(other.memory),
            count: self.count.saturating_add(other.count),
        }
    }
}

/// One sample, by category. `total` is the three added up.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Usages {
    pub total: Usage,
    pub app: Usage,
    pub core: Usage,
    pub tools: Usage,
}

/// Aggregates the app's tree and herdr-core's. None when the app's own pid is
/// not in `rows` (nothing to report, which is not zero).
///
/// `core`: the server's and holders' pids; ones not in `rows` are skipped. The
/// core is walked first and never into the app: Yuzora started from a herdr
/// terminal is still the app, and what it starts is still its tools. A core
/// that runs under the app (started by its `hc connect`) is still the core.
pub fn aggregate(rows: &[Row], app: u32, core: &[u32]) -> Option<Usages> {
    let mut by_pid: HashMap<u32, &Row> = HashMap::new();
    let mut children: HashMap<u32, Vec<&Row>> = HashMap::new();
    for row in rows {
        if by_pid.contains_key(&row.pid) {
            continue;
        }
        by_pid.insert(row.pid, row);
        if let Some(parent) = row.parent {
            children.entry(parent).or_default().push(row);
        }
    }
    let app_row = by_pid.get(&app)?;

    let mut seen = HashSet::from([app]);
    let mut own = Usage::default();
    own.add(app_row);
    let core_roots: Vec<&Row> = core
        .iter()
        .filter_map(|pid| by_pid.get(pid).copied())
        .collect();
    let core = walk(core_roots, &children, &mut seen);
    let tools = walk(
        children.get(&app).cloned().unwrap_or_default(),
        &children,
        &mut seen,
    );
    Some(Usages {
        total: own.plus(core).plus(tools),
        app: own,
        core,
        tools,
    })
}

/// Breadth first from `starts`, skipping (and not entering) what's seen.
fn walk<'a>(
    starts: Vec<&'a Row>,
    children: &HashMap<u32, Vec<&'a Row>>,
    seen: &mut HashSet<u32>,
) -> Usage {
    let mut usage = Usage::default();
    let mut queue: VecDeque<&Row> = VecDeque::new();
    for row in starts {
        if seen.insert(row.pid) {
            usage.add(row);
            queue.push_back(row);
        }
    }
    while let Some(row) = queue.pop_front() {
        for kid in children
            .get(&row.pid)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            if seen.insert(kid.pid) {
                usage.add(kid);
                queue.push_back(kid);
            }
        }
    }
    usage
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, parent: Option<u32>, cpu: f32, memory: u64) -> Row {
        Row {
            pid,
            parent,
            cpu,
            memory,
        }
    }

    /// The categories add up to the total, whatever got classified where.
    fn assert_conserved(u: &Usages) {
        assert_eq!(
            u.app.memory + u.core.memory + u.tools.memory,
            u.total.memory,
            "memory adds up"
        );
        assert_eq!(
            u.app.count + u.core.count + u.tools.count,
            u.total.count,
            "counts add up"
        );
        assert!(
            (u.app.cpu + u.core.cpu + u.tools.cpu - u.total.cpu).abs() < 1e-4,
            "cpu adds up"
        );
        assert_eq!(u.app.count, 1, "the app is one process");
    }

    #[test]
    fn nothing_when_the_app_is_not_in_the_table() {
        assert!(aggregate(&[row(1, None, 1.0, 100)], 999, &[]).is_none());
    }

    #[test]
    fn a_lone_app_is_the_whole_total() {
        // A sibling (20) is not the app's.
        let rows = [row(10, Some(1), 12.5, 4_096), row(20, Some(1), 99.0, 8_192)];
        let u = aggregate(&rows, 10, &[]).unwrap();
        let app = Usage {
            cpu: 12.5,
            memory: 4_096,
            count: 1,
        };
        assert_eq!(
            u,
            Usages {
                total: app,
                app,
                core: Usage::default(),
                tools: Usage::default()
            }
        );
    }

    #[test]
    fn grandchildren_are_tools_and_unrelated_trees_are_not_counted() {
        // 10 → 20 → 30; 40 → 41 belongs to nobody we measure.
        let rows = [
            row(10, Some(1), 1.0, 100),
            row(20, Some(10), 2.0, 200),
            row(30, Some(20), 4.0, 400),
            row(40, Some(1), 8.0, 800),
            row(41, Some(40), 16.0, 1_600),
        ];
        let u = aggregate(&rows, 10, &[]).unwrap();
        assert_eq!(
            u.tools,
            Usage {
                cpu: 6.0,
                memory: 600,
                count: 2
            }
        );
        assert_eq!(
            u.total,
            Usage {
                cpu: 7.0,
                memory: 700,
                count: 3
            }
        );
        assert_conserved(&u);
    }

    #[test]
    fn each_pid_counts_once() {
        let rows = [
            row(10, Some(1), 1.0, 100),
            row(20, Some(10), 2.0, 200),
            row(20, Some(10), 2.0, 200),
        ];
        let u = aggregate(&rows, 10, &[]).unwrap();
        assert_eq!(u.total.count, 2);
        assert_eq!(u.total.memory, 300);
    }

    #[test]
    fn a_parent_cycle_ends() {
        // 10 → 20 → 30 → 10.
        let rows = [
            row(10, Some(30), 1.0, 100),
            row(20, Some(10), 2.0, 200),
            row(30, Some(20), 4.0, 400),
        ];
        let u = aggregate(&rows, 10, &[]).unwrap();
        assert_eq!(u.tools.count, 2);
        assert_eq!(u.total.memory, 700);
        assert_conserved(&u);
    }

    #[test]
    fn an_exited_process_leaves_the_total() {
        let alive = [row(10, Some(1), 1.0, 100), row(20, Some(10), 2.0, 200)];
        let exited = [row(10, Some(1), 1.0, 100)];
        assert_eq!(aggregate(&alive, 10, &[]).unwrap().tools.count, 1);
        let after = aggregate(&exited, 10, &[]).unwrap();
        assert_eq!(after.tools.count, 0);
        assert_eq!(after.total.memory, after.app.memory);
    }

    #[test]
    fn the_core_daemon_and_its_terminals_are_core() {
        // The app (10) runs `hc connect` (11) and git (12 → 13). hc serve (50) is
        // launchd's, its holder (51) runs two shells (52, 53) and an agent (54).
        // Another session's core (60 → 61) is not this app's.
        let rows = [
            row(1, None, 0.0, 1),
            row(10, Some(1), 5.0, 1_000),
            row(11, Some(10), 0.5, 10),
            row(12, Some(10), 1.0, 20),
            row(13, Some(12), 1.0, 30),
            row(50, Some(1), 2.0, 500),
            row(51, Some(50), 0.5, 50),
            row(52, Some(51), 0.0, 5),
            row(53, Some(51), 0.0, 5),
            row(54, Some(53), 30.0, 300),
            row(60, Some(1), 9.0, 900),
            row(61, Some(60), 9.0, 900),
        ];
        let u = aggregate(&rows, 10, &[50, 51]).unwrap();
        assert_eq!(
            u.app,
            Usage {
                cpu: 5.0,
                memory: 1_000,
                count: 1
            }
        );
        assert_eq!(
            u.core,
            Usage {
                cpu: 32.5,
                memory: 860,
                count: 5
            }
        );
        assert_eq!(
            u.tools,
            Usage {
                cpu: 2.5,
                memory: 60,
                count: 3
            }
        );
        assert_eq!(
            u.total,
            Usage {
                cpu: 40.0,
                memory: 1_920,
                count: 9
            }
        );
        assert_conserved(&u);
    }

    #[test]
    fn a_holder_left_by_a_restart_still_counts_as_core() {
        // After `hc server restart` an old holder (51) drains under launchd, no
        // longer under the new server (55); status still lists it.
        let rows = [
            row(10, Some(1), 1.0, 100),
            row(51, Some(1), 1.0, 50),
            row(52, Some(51), 1.0, 5),
            row(55, Some(1), 1.0, 500),
            row(56, Some(55), 1.0, 50),
        ];
        let u = aggregate(&rows, 10, &[55, 51]).unwrap();
        assert_eq!(u.core.count, 4);
        assert_eq!(u.core.memory, 605);
        assert_conserved(&u);
    }

    #[test]
    fn yuzora_started_in_a_herdr_terminal_stays_the_app() {
        // hc serve (50) → holder (51) → zsh (52) → the app (10) → git (12).
        let rows = [
            row(50, Some(1), 1.0, 500),
            row(51, Some(50), 1.0, 50),
            row(52, Some(51), 1.0, 5),
            row(10, Some(52), 4.0, 1_000),
            row(12, Some(10), 2.0, 20),
        ];
        let u = aggregate(&rows, 10, &[50, 51]).unwrap();
        assert_eq!(
            u.core,
            Usage {
                cpu: 3.0,
                memory: 555,
                count: 3
            }
        );
        assert_eq!(
            u.tools,
            Usage {
                cpu: 2.0,
                memory: 20,
                count: 1
            }
        );
        assert_eq!(u.app.memory, 1_000);
        assert_conserved(&u);
    }

    #[test]
    fn a_core_under_the_app_is_still_core() {
        // Started by the app's `hc connect` (11), the server runs under the app.
        let rows = [
            row(10, Some(1), 1.0, 100),
            row(11, Some(10), 1.0, 10),
            row(50, Some(11), 2.0, 500),
            row(52, Some(50), 3.0, 5),
        ];
        let u = aggregate(&rows, 10, &[50]).unwrap();
        assert_eq!(
            u.core,
            Usage {
                cpu: 5.0,
                memory: 505,
                count: 2
            }
        );
        assert_eq!(
            u.tools,
            Usage {
                cpu: 1.0,
                memory: 10,
                count: 1
            }
        );
        assert_conserved(&u);
    }

    #[test]
    fn a_core_root_that_is_gone_or_the_app_counts_nothing() {
        let rows = [row(10, Some(1), 1.0, 100), row(20, Some(10), 2.0, 200)];
        let u = aggregate(&rows, 10, &[999, 10]).unwrap();
        assert_eq!(u.core, Usage::default());
        assert_eq!(u.tools.count, 1);
        assert_conserved(&u);
    }
}

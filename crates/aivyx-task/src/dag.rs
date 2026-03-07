//! DAG (Directed Acyclic Graph) utilities for step dependency resolution.
//!
//! When steps declare `depends_on` relationships, they form a DAG. This module
//! provides validation (cycle detection), ready-step resolution (which steps
//! can execute now), failure propagation (skipping downstream dependents),
//! and topological ordering.

use aivyx_core::{AivyxError, Result};

use crate::types::{Step, StepStatus};

/// Validate that the step dependencies form a valid DAG (no cycles, valid indices).
///
/// Uses Kahn's algorithm for topological sorting — if the sort doesn't consume
/// all nodes, a cycle exists.
pub fn validate_dag(steps: &[Step]) -> Result<()> {
    let n = steps.len();

    // Check all dependency indices are valid
    for step in steps {
        for &dep in &step.depends_on {
            if dep >= n {
                return Err(AivyxError::Task(format!(
                    "step {} depends on non-existent step {dep}",
                    step.index
                )));
            }
            if dep == step.index {
                return Err(AivyxError::Task(format!(
                    "step {} depends on itself",
                    step.index
                )));
            }
        }
    }

    // Kahn's algorithm: count in-degrees, process zero-degree nodes
    let mut in_degree = vec![0usize; n];
    let mut adj: Vec<Vec<usize>> = vec![vec![]; n];

    for step in steps {
        in_degree[step.index] = step.depends_on.len();
        for &dep in &step.depends_on {
            adj[dep].push(step.index);
        }
    }

    let mut queue: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    let mut visited = 0;

    while let Some(node) = queue.pop() {
        visited += 1;
        for &dependent in &adj[node] {
            in_degree[dependent] -= 1;
            if in_degree[dependent] == 0 {
                queue.push(dependent);
            }
        }
    }

    if visited != n {
        Err(AivyxError::Task("step dependencies contain a cycle".into()))
    } else {
        Ok(())
    }
}

/// Return the indices of steps that are ready to execute.
///
/// A step is ready when:
/// - Its status is `Pending`
/// - All steps in its `depends_on` list have status `Completed`
pub fn ready_steps(steps: &[Step]) -> Vec<usize> {
    steps
        .iter()
        .filter(|s| matches!(s.status, StepStatus::Pending))
        .filter(|s| {
            s.depends_on.iter().all(|&dep| {
                steps
                    .get(dep)
                    .is_some_and(|d| matches!(d.status, StepStatus::Completed))
            })
        })
        .map(|s| s.index)
        .collect()
}

/// Mark all transitive dependents of a failed step as `Skipped`.
///
/// When a step fails, any step that transitively depends on it cannot proceed.
/// This walks the dependency graph forward from the failed step and marks
/// all reachable pending/running steps as `Skipped`.
pub fn skip_downstream(steps: &mut [Step], failed_idx: usize) {
    let n = steps.len();

    // Build forward adjacency: step → list of dependents
    let mut adj: Vec<Vec<usize>> = vec![vec![]; n];
    for step in steps.iter() {
        for &dep in &step.depends_on {
            if dep < n {
                adj[dep].push(step.index);
            }
        }
    }

    // BFS from the failed step
    let mut queue = vec![failed_idx];
    let mut visited = vec![false; n];
    visited[failed_idx] = true;

    while let Some(node) = queue.pop() {
        for &dependent in &adj[node] {
            if !visited[dependent] {
                visited[dependent] = true;
                if matches!(
                    steps[dependent].status,
                    StepStatus::Pending | StepStatus::Running
                ) {
                    steps[dependent].status = StepStatus::Skipped;
                }
                queue.push(dependent);
            }
        }
    }
}

/// Return a topological ordering of steps (indices).
///
/// Steps with no dependencies come first; steps that depend on others
/// come after their dependencies. Returns an error if a cycle is detected.
pub fn topological_order(steps: &[Step]) -> Result<Vec<usize>> {
    let n = steps.len();
    let mut in_degree = vec![0usize; n];
    let mut adj: Vec<Vec<usize>> = vec![vec![]; n];

    for step in steps {
        in_degree[step.index] = step.depends_on.len();
        for &dep in &step.depends_on {
            adj[dep].push(step.index);
        }
    }

    let mut queue: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    // Sort initial queue for deterministic ordering
    queue.sort_unstable();
    let mut order = Vec::with_capacity(n);

    while let Some(node) = queue.pop() {
        order.push(node);
        let mut new_ready = Vec::new();
        for &dependent in &adj[node] {
            in_degree[dependent] -= 1;
            if in_degree[dependent] == 0 {
                new_ready.push(dependent);
            }
        }
        new_ready.sort_unstable();
        queue.extend(new_ready);
    }

    if order.len() != n {
        Err(AivyxError::Task("step dependencies contain a cycle".into()))
    } else {
        Ok(order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StepKind;

    fn make_step(index: usize, depends_on: Vec<usize>) -> Step {
        Step {
            index,
            description: format!("step {index}"),
            tool_hints: vec![],
            status: StepStatus::Pending,
            prompt: None,
            result: None,
            retries: 0,
            started_at: None,
            completed_at: None,
            depends_on,
            kind: StepKind::default(),
        }
    }

    #[test]
    fn validate_dag_simple_chain() {
        let steps = vec![
            make_step(0, vec![]),
            make_step(1, vec![0]),
            make_step(2, vec![1]),
        ];
        assert!(validate_dag(&steps).is_ok());
    }

    #[test]
    fn validate_dag_parallel_then_join() {
        // 0 and 1 are independent; 2 depends on both
        let steps = vec![
            make_step(0, vec![]),
            make_step(1, vec![]),
            make_step(2, vec![0, 1]),
        ];
        assert!(validate_dag(&steps).is_ok());
    }

    #[test]
    fn validate_dag_cycle_detected() {
        let steps = vec![make_step(0, vec![1]), make_step(1, vec![0])];
        let err = validate_dag(&steps).unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn validate_dag_self_dependency() {
        let steps = vec![make_step(0, vec![0])];
        let err = validate_dag(&steps).unwrap_err();
        assert!(err.to_string().contains("depends on itself"));
    }

    #[test]
    fn validate_dag_invalid_index() {
        let steps = vec![make_step(0, vec![5])];
        let err = validate_dag(&steps).unwrap_err();
        assert!(err.to_string().contains("non-existent"));
    }

    #[test]
    fn validate_dag_empty_is_valid() {
        assert!(validate_dag(&[]).is_ok());
    }

    #[test]
    fn ready_steps_all_independent() {
        let steps = vec![make_step(0, vec![]), make_step(1, vec![])];
        assert_eq!(ready_steps(&steps), vec![0, 1]);
    }

    #[test]
    fn ready_steps_chain_only_first() {
        let steps = vec![
            make_step(0, vec![]),
            make_step(1, vec![0]),
            make_step(2, vec![1]),
        ];
        assert_eq!(ready_steps(&steps), vec![0]);
    }

    #[test]
    fn ready_steps_after_completion() {
        let mut steps = vec![
            make_step(0, vec![]),
            make_step(1, vec![0]),
            make_step(2, vec![0]),
        ];
        steps[0].status = StepStatus::Completed;
        assert_eq!(ready_steps(&steps), vec![1, 2]);
    }

    #[test]
    fn ready_steps_partial_deps() {
        let mut steps = vec![
            make_step(0, vec![]),
            make_step(1, vec![]),
            make_step(2, vec![0, 1]),
        ];
        // Only step 0 completed; step 2 still blocked on step 1
        steps[0].status = StepStatus::Completed;
        assert_eq!(ready_steps(&steps), vec![1]);
    }

    #[test]
    fn skip_downstream_propagates() {
        let mut steps = vec![
            make_step(0, vec![]),
            make_step(1, vec![0]),
            make_step(2, vec![1]),
            make_step(3, vec![]), // independent, should not be skipped
        ];
        steps[0].status = StepStatus::Failed {
            reason: "error".into(),
        };
        skip_downstream(&mut steps, 0);

        assert!(matches!(steps[0].status, StepStatus::Failed { .. }));
        assert_eq!(steps[1].status, StepStatus::Skipped);
        assert_eq!(steps[2].status, StepStatus::Skipped);
        assert_eq!(steps[3].status, StepStatus::Pending); // untouched
    }

    #[test]
    fn skip_downstream_diamond() {
        //   0
        //  / \
        // 1   2
        //  \ /
        //   3
        let mut steps = vec![
            make_step(0, vec![]),
            make_step(1, vec![0]),
            make_step(2, vec![0]),
            make_step(3, vec![1, 2]),
        ];
        steps[0].status = StepStatus::Failed {
            reason: "error".into(),
        };
        skip_downstream(&mut steps, 0);

        assert_eq!(steps[1].status, StepStatus::Skipped);
        assert_eq!(steps[2].status, StepStatus::Skipped);
        assert_eq!(steps[3].status, StepStatus::Skipped);
    }

    #[test]
    fn topological_order_simple() {
        let steps = vec![
            make_step(0, vec![]),
            make_step(1, vec![0]),
            make_step(2, vec![0]),
            make_step(3, vec![1, 2]),
        ];
        let order = topological_order(&steps).unwrap();
        // 0 must come before 1 and 2; 3 must come after both
        let pos = |idx: usize| order.iter().position(|&x| x == idx).unwrap();
        assert!(pos(0) < pos(1));
        assert!(pos(0) < pos(2));
        assert!(pos(1) < pos(3));
        assert!(pos(2) < pos(3));
    }

    #[test]
    fn topological_order_cycle_fails() {
        let steps = vec![make_step(0, vec![1]), make_step(1, vec![0])];
        assert!(topological_order(&steps).is_err());
    }

    #[test]
    fn topological_order_all_independent() {
        let steps = vec![
            make_step(0, vec![]),
            make_step(1, vec![]),
            make_step(2, vec![]),
        ];
        let order = topological_order(&steps).unwrap();
        assert_eq!(order.len(), 3);
    }
}

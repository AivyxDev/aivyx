use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

fn generate_plan_json(step_count: usize) -> String {
    let steps: Vec<String> = (0..step_count)
        .map(|i| {
            format!(
                r#"{{"description": "Step {i}: perform action on the data", "tool_hints": ["web_search", "file_write"]}}"#
            )
        })
        .collect();
    format!("[{}]", steps.join(",\n"))
}

fn generate_fenced_plan_json(step_count: usize) -> String {
    format!("```json\n{}\n```", generate_plan_json(step_count))
}

/// Generate a DAG plan JSON where steps form a diamond pattern:
/// Steps 0..width are independent roots, step `width` depends on all of them,
/// then another layer of independent steps, etc.
fn generate_dag_plan_json(step_count: usize) -> String {
    let width = 3; // parallel width
    let steps: Vec<String> = (0..step_count)
        .map(|i| {
            let depends_on = if i == 0 || i % (width + 1) != width {
                // Independent step (root or parallel layer)
                "[]".to_string()
            } else {
                // Join step: depends on the previous `width` steps
                let deps: Vec<String> = (i.saturating_sub(width)..i)
                    .map(|d| d.to_string())
                    .collect();
                format!("[{}]", deps.join(", "))
            };
            format!(
                r#"{{"description": "Step {i}: DAG action", "tool_hints": ["shell"], "depends_on": {depends_on}}}"#
            )
        })
        .collect();
    format!("[{}]", steps.join(",\n"))
}

fn bench_parse_plan_response(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse_plan_response");

    for step_count in [4, 10, 25, 50] {
        let json = generate_plan_json(step_count);
        group.bench_with_input(
            BenchmarkId::new("raw_json", step_count),
            &json,
            |b, input| {
                b.iter(|| aivyx_task::planner::parse_plan_response(black_box(input)).unwrap());
            },
        );

        let fenced = generate_fenced_plan_json(step_count);
        group.bench_with_input(
            BenchmarkId::new("fenced_json", step_count),
            &fenced,
            |b, input| {
                b.iter(|| aivyx_task::planner::parse_plan_response(black_box(input)).unwrap());
            },
        );

        let dag_json = generate_dag_plan_json(step_count);
        group.bench_with_input(
            BenchmarkId::new("dag_json", step_count),
            &dag_json,
            |b, input| {
                b.iter(|| aivyx_task::planner::parse_plan_response(black_box(input)).unwrap());
            },
        );
    }

    group.finish();
}

fn bench_mission_methods(c: &mut Criterion) {
    let mut group = c.benchmark_group("mission_methods");

    for step_count in [5, 20, 50] {
        let json = generate_plan_json(step_count);
        let steps = aivyx_task::planner::parse_plan_response(&json).unwrap();

        let mut mission = aivyx_task::Mission::new("Benchmark mission", "researcher");
        mission.steps = steps;

        // Mark half the steps as completed
        for step in mission.steps.iter_mut().take(step_count / 2) {
            step.status = aivyx_task::StepStatus::Completed;
            step.result = Some("done".to_string());
        }

        group.bench_with_input(
            BenchmarkId::new("next_pending_step", step_count),
            &mission,
            |b, m| {
                b.iter(|| black_box(m.next_pending_step()));
            },
        );

        group.bench_with_input(
            BenchmarkId::new("completed_step_summaries", step_count),
            &mission,
            |b, m| {
                b.iter(|| black_box(m.completed_step_summaries()));
            },
        );

        group.bench_with_input(
            BenchmarkId::new("execution_mode", step_count),
            &mission,
            |b, m| {
                b.iter(|| black_box(m.execution_mode()));
            },
        );
    }

    group.finish();
}

fn bench_dag_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("dag_operations");

    for step_count in [4, 10, 25, 50] {
        let dag_json = generate_dag_plan_json(step_count);
        let steps = aivyx_task::planner::parse_plan_response(&dag_json).unwrap();

        group.bench_with_input(
            BenchmarkId::new("validate_dag", step_count),
            &steps,
            |b, steps| {
                b.iter(|| aivyx_task::dag::validate_dag(black_box(steps)).unwrap());
            },
        );

        group.bench_with_input(
            BenchmarkId::new("topological_order", step_count),
            &steps,
            |b, steps| {
                b.iter(|| aivyx_task::dag::topological_order(black_box(steps)).unwrap());
            },
        );

        // Benchmark ready_steps with half-completed DAG
        let mut half_done_steps = steps.clone();
        for step in half_done_steps.iter_mut().take(step_count / 2) {
            step.status = aivyx_task::StepStatus::Completed;
        }
        group.bench_with_input(
            BenchmarkId::new("ready_steps", step_count),
            &half_done_steps,
            |b, steps| {
                b.iter(|| black_box(aivyx_task::dag::ready_steps(steps)));
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_parse_plan_response,
    bench_mission_methods,
    bench_dag_operations,
);
criterion_main!(benches);

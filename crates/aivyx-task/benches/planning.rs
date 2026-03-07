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
    }

    group.finish();
}

criterion_group!(benches, bench_parse_plan_response, bench_mission_methods);
criterion_main!(benches);

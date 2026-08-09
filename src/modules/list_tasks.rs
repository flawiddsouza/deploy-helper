use crate::common::Task;
use crate::modules::filter::{self, Decision, FilterConfig, GateState};
use crate::modules::include_tasks;
use crate::utils;
use indexmap::IndexMap;
use serde_json::Value;
use std::path::Path;

pub fn format_line(indent: usize, name: &str, name_col_width: usize, tags: &[String]) -> String {
    let pad = "  ".repeat(indent);
    let tags_str = format!("[{}]", tags.join(", "));
    format!(
        "{}{:<width$}  TAGS: {}",
        pad,
        name,
        tags_str,
        width = name_col_width
    )
}

pub fn run(
    deployments: &[crate::Deployment],
    config: &FilterConfig,
    deploy_file_dir: &Path,
    extra_vars_map: &IndexMap<String, Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut state = GateState::new(config);
    let mut working_vars = extra_vars_map.clone();
    for dep in deployments {
        crate::modules::vars_file::load_all(
            &dep.vars_files,
            deploy_file_dir,
            &mut working_vars,
            extra_vars_map,
        )?;
        crate::apply_deployment_vars(dep.vars.as_ref(), &mut working_vars, extra_vars_map)?;
        let dep_name = utils::replace_placeholders(&dep.name, &working_vars);
        println!("Starting deployment: {}", dep_name);
        let ancestor = dep.tags.clone().unwrap_or_default();
        let recovery_ancestor = filter::merge_tags(&ancestor, Some(&["always".to_string()]));
        let mut visible = collect_visible(
            &dep.tasks,
            &ancestor,
            config,
            &mut state,
            0,
            deploy_file_dir,
            &mut working_vars,
        )?;
        let on_failure_visible = collect_visible(
            &dep.on_failure,
            &recovery_ancestor,
            config,
            &mut state,
            0,
            deploy_file_dir,
            &mut working_vars,
        )?;
        visible.extend(
            on_failure_visible
                .into_iter()
                .map(|(depth, name, tags)| (depth, format!("[on_failure] {}", name), tags)),
        );
        let always_visible = collect_visible(
            &dep.always,
            &recovery_ancestor,
            config,
            &mut state,
            0,
            deploy_file_dir,
            &mut working_vars,
        )?;
        visible.extend(
            always_visible
                .into_iter()
                .map(|(depth, name, tags)| (depth, format!("[always] {}", name), tags)),
        );
        let width = visible
            .iter()
            .map(|(_, name, _)| name.chars().count())
            .max()
            .unwrap_or(0);
        for (indent, name, tags) in visible {
            println!("{}", format_line(indent + 1, &name, width, &tags));
        }
        println!();
    }
    Ok(())
}

fn collect_visible(
    tasks: &[Task],
    ancestor_tags: &[String],
    config: &FilterConfig,
    state: &mut GateState,
    depth: usize,
    deploy_file_dir: &Path,
    vars_map: &mut IndexMap<String, Value>,
) -> Result<Vec<(usize, String, Vec<String>)>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    for task in tasks {
        let task_name = utils::replace_placeholders(&task.name, vars_map);
        let effective = filter::merge_tags(ancestor_tags, task.tags.as_deref());
        if matches!(
            filter::decide(&task_name, &effective, config, state),
            Decision::Skip(_)
        ) {
            continue;
        }
        out.push((depth, task_name.clone(), effective.clone()));

        if let Some(include_file) = &task.include_tasks {
            if let Some(task_vars) = &task.vars {
                for (key, value) in task_vars {
                    let value_evaluated = utils::replace_placeholders_value_result(
                        value,
                        vars_map,
                        task.no_log.unwrap_or(false),
                    )
                    .map_err(|error| utils::task_error(&task_name, error))?;
                    vars_map.insert(key.clone(), value_evaluated);
                }
            }
            let include_path = deploy_file_dir.join(include_file);
            let children = include_tasks::process(include_path.to_str().unwrap());
            let mut nested = collect_visible(
                &children,
                &effective,
                config,
                state,
                depth + 1,
                deploy_file_dir,
                vars_map,
            )?;
            out.append(&mut nested);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_line_pads_name_to_column_width() {
        let line = format_line(1, "A", 10, &["x".to_string(), "y".to_string()]);
        assert_eq!(line, "  A           TAGS: [x, y]");
    }

    #[test]
    fn format_line_empty_tags() {
        let line = format_line(1, "A", 5, &[]);
        assert_eq!(line, "  A      TAGS: []");
    }

    #[test]
    fn format_line_indent_scales() {
        let line = format_line(3, "A", 1, &["t".to_string()]);
        assert!(line.starts_with("      A"));
    }
}

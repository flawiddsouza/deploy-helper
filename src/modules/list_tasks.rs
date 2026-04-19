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
    vars_map: &IndexMap<String, Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut state = GateState::new(config);
    let mut working_vars = vars_map.clone();
    for dep in deployments {
        if let Some(dep_vars) = &dep.vars {
            for (key, value) in dep_vars {
                let evaluated = utils::replace_placeholders_vars(value, &working_vars);
                working_vars.insert(key.clone(), evaluated);
            }
        }
        let dep_name = utils::replace_placeholders(&dep.name, &working_vars);
        println!("Starting deployment: {}", dep_name);
        let ancestor = dep.tags.clone().unwrap_or_default();
        let visible = collect_visible(
            &dep.tasks,
            &ancestor,
            config,
            &mut state,
            0,
            deploy_file_dir,
            &working_vars,
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
    vars_map: &IndexMap<String, Value>,
) -> Vec<(usize, String, Vec<String>)> {
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
        out.push((depth, task_name, effective.clone()));

        if let Some(include_file) = &task.include_tasks {
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
            );
            out.append(&mut nested);
        }
    }
    out
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

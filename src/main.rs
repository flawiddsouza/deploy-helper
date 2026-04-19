mod common;
mod modules;
mod utils;

use clap::{Arg, Command as ClapCommand};
use colored::Colorize;
use indexmap::IndexMap;
use modules::filter;
use serde::Deserialize;
use serde_json::Value;
use ssh2::Session;
use std::path::Path;
use std::process::exit;

#[derive(Debug, Deserialize)]
struct ServerConfig {
    hosts: IndexMap<String, TargetHost>,
}

#[derive(Debug, Deserialize)]
struct TargetHost {
    host: String,
    port: Option<u16>,
    user: Option<String>,
    password: Option<String>,
    ssh_key_path: Option<String>,
}

impl TargetHost {
    fn resolve(&self, vars: &IndexMap<String, Value>) -> Self {
        TargetHost {
            host: utils::replace_placeholders(&self.host, vars),
            port: self.port,
            user: self
                .user
                .as_deref()
                .map(|s| utils::replace_placeholders(s, vars)),
            password: self
                .password
                .as_deref()
                .map(|s| utils::replace_placeholders(s, vars)),
            ssh_key_path: self
                .ssh_key_path
                .as_deref()
                .map(|s| utils::replace_placeholders(s, vars)),
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct Deployment {
    pub(crate) name: String,
    pub(crate) hosts: String,
    pub(crate) chdir: Option<String>,
    pub(crate) login_shell: Option<bool>,
    pub(crate) vars: Option<IndexMap<String, String>>,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) tasks: Vec<common::Task>,
}

struct RunContext<'a> {
    is_localhost: bool,
    session: Option<&'a Session>,
    vars_map: &'a mut IndexMap<String, Value>,
    deploy_file_dir: &'a Path,
    become_password: &'a mut Option<String>,
    filter_config: &'a filter::FilterConfig,
    filter_state: &'a mut filter::GateState,
    step_state: &'a mut modules::step::StepState,
}

fn process_tasks(
    ctx: &mut RunContext,
    tasks: &[common::Task],
    dep_chdir: Option<&str>,
    dep_login_shell: bool,
    ancestor_tags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    for task in tasks {
        let task_name = utils::replace_placeholders(&task.name, ctx.vars_map);

        let effective_tags = filter::merge_tags(ancestor_tags, task.tags.as_deref());

        match filter::decide(
            &task_name,
            &effective_tags,
            ctx.filter_config,
            ctx.filter_state,
        ) {
            filter::Decision::Run => {}
            filter::Decision::Skip(_) => continue,
        }

        if !modules::when::process(&task.when, ctx.vars_map) {
            println!("{}", format!("Skipping task: {}\n", task_name).yellow());
            continue;
        }

        if ctx.step_state.should_prompt() {
            match modules::step::prompt(&task_name)? {
                modules::step::StepChoice::Run => {}
                modules::step::StepChoice::Skip => {
                    println!(
                        "{}",
                        format!("Skipping task: {} (step)\n", task_name).yellow()
                    );
                    continue;
                }
                modules::step::StepChoice::ContinueWithoutPrompt => {
                    ctx.step_state.continue_in_deployment = true;
                }
            }
        }

        println!("{}", format!("Executing task: {}", task_name).cyan());

        if let Some(vars) = &task.vars {
            for (key, value) in vars {
                let evaluated_value = utils::replace_placeholders_vars(&value, ctx.vars_map);
                ctx.vars_map.insert(key.clone(), evaluated_value);
            }
        }

        // Use task-level chdir if present, otherwise use top-level chdir
        let task_chdir = task
            .chdir
            .as_deref()
            .or(dep_chdir)
            .map(|s| utils::replace_placeholders(s, ctx.vars_map));

        let use_login_shell = task.login_shell.unwrap_or(dep_login_shell);

        let task_become = task.r#become.unwrap_or(false);
        let task_become_method = task.become_method.as_deref().unwrap_or("sudo").to_string();

        // Validate become_method early before asking for password
        if task_become && !matches!(task_become_method.as_str(), "sudo" | "doas" | "su") {
            return Err(format!(
                "Unsupported become_method '{}'. Supported values: sudo, doas, su.",
                task_become_method
            )
            .into());
        }

        if task_become {
            if task_become_method == "doas" {
                if ctx
                    .vars_map
                    .get("become_password")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .is_some()
                {
                    return Err(
                        "become_method 'doas' does not support password via become_password (doas requires a tty). Configure passwordless doas or use sudo/su instead."
                            .into(),
                    );
                }
            } else if ctx.become_password.is_none() {
                if let Some(pw) = ctx.vars_map.get("become_password").and_then(|v| v.as_str()) {
                    *ctx.become_password = Some(pw.to_string());
                } else {
                    *ctx.become_password = Some(rpassword::prompt_password("BECOME password: ")?);
                }
            }
        }

        let loop_items = task.r#loop.clone().unwrap_or_else(|| vec![Value::Null]);

        for item in loop_items {
            ctx.vars_map.shift_remove("item");

            if !item.is_null() {
                ctx.vars_map.insert("item".to_string(), item.clone());
            }

            if let Some(debug) = &task.debug {
                modules::debug::process(debug, ctx.vars_map);
            }

            let task_become_password = if task_become_method == "doas" {
                None
            } else {
                ctx.become_password.as_deref().filter(|s| !s.is_empty())
            };

            if let Some(shell_command) = &task.shell {
                let display_segments = utils::split_commands(shell_command);
                modules::command::process_shell_block(
                    shell_command,
                    display_segments,
                    ctx.is_localhost,
                    ctx.session,
                    task_chdir.as_deref(),
                    task.register.as_ref(),
                    use_login_shell,
                    ctx.vars_map,
                    task_become,
                    &task_become_method,
                    task_become_password,
                )?;
            }

            if let Some(command) = &task.command {
                let commands = utils::split_commands(command);
                modules::command::process_command(
                    commands,
                    ctx.is_localhost,
                    ctx.session,
                    task_chdir.as_deref(),
                    task.register.as_ref(),
                    use_login_shell,
                    ctx.vars_map,
                    task_become,
                    &task_become_method,
                    task_become_password,
                )?;
            }

            if let Some(spec) = &task.template {
                modules::template::process(
                    spec,
                    ctx.deploy_file_dir,
                    ctx.is_localhost,
                    ctx.session,
                    ctx.vars_map,
                    task_become,
                    &task_become_method,
                    task_become_password,
                    task.register.as_ref(),
                )?;
            }

            if let Some(spec) = &task.copy {
                modules::copy::process(
                    &task_name,
                    spec,
                    ctx.deploy_file_dir,
                    ctx.is_localhost,
                    ctx.session,
                    ctx.vars_map,
                    task_become,
                    &task_become_method,
                    task_become_password,
                    task.register.as_ref(),
                )?;
            }

            if let Some(include_file) = &task.include_tasks {
                println!(
                    "{}",
                    format!("Including tasks from: {}\n", include_file).blue()
                );
                let include_file_path = ctx.deploy_file_dir.join(include_file);
                let included_tasks =
                    modules::include_tasks::process(include_file_path.to_str().unwrap());
                process_tasks(
                    ctx,
                    &included_tasks,
                    task_chdir.as_deref(),
                    use_login_shell,
                    &effective_tags,
                )?;
            }
        }

        println!();
    }

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let matches = ClapCommand::new("deploy-helper")
        .version("1.0.3")
        .about("Deployment helper tool")
        .arg(
            Arg::new("deploy_file")
                .help("The deployment YAML file")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::new("extra_vars")
                .short('e')
                .long("extra-vars")
                .value_name("VARS")
                .help("Set additional variables as key=value, JSON, or @file. Can be specified multiple times.")
                .num_args(1)
                .action(clap::ArgAction::Append),
        )
        .arg(
            Arg::new("server_file")
                .short('i')
                .long("inventory")
                .value_name("FILE")
                .help("The server configuration YAML file")
                .num_args(1),
        )
        .arg(
            Arg::new("tags")
                .short('t')
                .long("tags")
                .value_name("TAGS")
                .help("Only run tasks whose effective tags intersect this list (comma-separated; repeatable)")
                .num_args(1)
                .action(clap::ArgAction::Append),
        )
        .arg(
            Arg::new("skip_tags")
                .long("skip-tags")
                .value_name("TAGS")
                .help("Skip tasks whose effective tags intersect this list (wins over --tags)")
                .num_args(1)
                .action(clap::ArgAction::Append),
        )
        .arg(
            Arg::new("start_at_task")
                .long("start-at-task")
                .value_name("NAME")
                .help("Skip tasks until one whose name matches exactly; then run from there")
                .num_args(1),
        )
        .arg(
            Arg::new("step")
                .long("step")
                .help("Prompt before each task")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("list_tasks")
                .long("list-tasks")
                .help("Print what would run, then exit without running")
                .action(clap::ArgAction::SetTrue),
        )
        .get_matches();

    let deploy_file = matches.get_one::<String>("deploy_file").unwrap();
    let extra_vars_list: Vec<&str> = matches
        .get_many::<String>("extra_vars")
        .unwrap_or_default()
        .map(|s| s.as_str())
        .collect();
    let default_server_file = "servers.yml".to_string();
    let server_file = matches
        .get_one::<String>("server_file")
        .unwrap_or(&default_server_file);

    fn split_tags(values: Vec<&str>) -> Vec<String> {
        values
            .into_iter()
            .flat_map(|v| v.split(','))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    let tags_raw: Vec<&str> = matches
        .get_many::<String>("tags")
        .unwrap_or_default()
        .map(|s| s.as_str())
        .collect();
    let skip_tags_raw: Vec<&str> = matches
        .get_many::<String>("skip_tags")
        .unwrap_or_default()
        .map(|s| s.as_str())
        .collect();

    let filter_config = filter::FilterConfig {
        tags: split_tags(tags_raw),
        skip_tags: split_tags(skip_tags_raw),
        start_at_task: matches.get_one::<String>("start_at_task").cloned(),
    };

    let step_enabled = matches.get_flag("step");
    let list_tasks_enabled = matches.get_flag("list_tasks");

    let server_config: ServerConfig = utils::read_yaml(server_file);
    let deployment_docs: Vec<Vec<Deployment>> = utils::read_yaml_multi(deploy_file);
    let deployments = deployment_docs.into_iter().flatten().collect::<Vec<_>>();

    let mut vars_map: IndexMap<String, Value> = IndexMap::new();

    for extra_vars in &extra_vars_list {
        if extra_vars.starts_with('@') {
            let extra_vars_file = &extra_vars[1..];
            let extra_vars_path = Path::new(extra_vars_file);
            if extra_vars_path.exists() {
                let yaml_vars: IndexMap<String, Value> = utils::read_yaml(extra_vars_file);
                vars_map.extend(yaml_vars);
            } else {
                eprintln!(
                    "{}",
                    format!("Extra vars file not found: {}", extra_vars_file).red()
                );
                exit(1);
            }
        } else if extra_vars.starts_with('{') {
            let json_vars: IndexMap<String, Value> = serde_json::from_str(extra_vars)?;
            vars_map.extend(json_vars);
        } else {
            for var in extra_vars.split(' ') {
                let parts: Vec<&str> = var.splitn(2, '=').collect();
                if parts.len() == 2 {
                    vars_map.insert(parts[0].to_string(), Value::String(parts[1].to_string()));
                }
            }
        }
    }

    let deploy_file_path = Path::new(deploy_file);
    let deploy_file_dir = deploy_file_path.parent().unwrap_or(Path::new("."));

    if list_tasks_enabled {
        modules::list_tasks::run(&deployments, &filter_config, deploy_file_dir, &vars_map)?;
        return Ok(());
    }

    let mut filter_state = filter::GateState::new(&filter_config);
    let mut step_state = modules::step::StepState::new(step_enabled);

    for dep in deployments {
        step_state.reset_for_deployment();

        if let Some(dep_vars) = &dep.vars {
            for (key, value) in dep_vars {
                let evaluated_value = utils::replace_placeholders_vars(&value, &vars_map);
                vars_map.insert(key.clone(), evaluated_value);
            }
        }

        let dep_name = utils::replace_placeholders(&dep.name, &vars_map);
        if let Some(chdir) = &dep.chdir {
            let resolved = utils::replace_placeholders(chdir, &vars_map);
            println!("{}", format!("Starting deployment: {}", dep_name).green());
            println!("{}", format!("(chdir: {})\n", resolved).bright_black());
        } else {
            println!("{}", format!("Starting deployment: {}\n", dep_name).green());
        }

        let dep_ancestor_tags: Vec<String> = dep.tags.clone().unwrap_or_default();

        let hosts: Vec<&str> = dep.hosts.split(',').map(|s| s.trim()).collect();

        let hosts_len = hosts.len();

        for host in hosts {
            if hosts_len > 1 {
                println!("{}", format!("Processing host: {}\n", host).blue());
            }

            let mut become_password: Option<String> = None;

            if let Some(target_host) = server_config.hosts.get(host) {
                let target_host = target_host.resolve(&vars_map);
                let is_localhost = target_host.host == "localhost";
                let session = if !is_localhost {
                    let port = target_host.port.unwrap_or(22); // Use default port 22 if not provided
                    let user = target_host
                        .user
                        .as_deref()
                        .ok_or("Missing user for remote host")?;
                    let password = target_host.password.as_deref();
                    let ssh_key_path = target_host.ssh_key_path.as_deref();

                    Some(utils::setup_ssh_session(
                        &target_host.host,
                        port,
                        user,
                        password,
                        ssh_key_path,
                    )?)
                } else {
                    None
                };

                let mut ctx = RunContext {
                    is_localhost,
                    session: session.as_ref(),
                    vars_map: &mut vars_map,
                    deploy_file_dir,
                    become_password: &mut become_password,
                    filter_config: &filter_config,
                    filter_state: &mut filter_state,
                    step_state: &mut step_state,
                };
                process_tasks(
                    &mut ctx,
                    &dep.tasks,
                    dep.chdir.as_deref(),
                    dep.login_shell.unwrap_or(false),
                    &dep_ancestor_tags,
                )?;
            } else {
                eprintln!(
                    "{}",
                    format!("No server config found for host: {}", host).red()
                );
            }
        }
    }

    Ok(())
}

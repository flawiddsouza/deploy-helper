use clap::{Arg, Command as ClapCommand};
use colored::*;
use indexmap::IndexMap;
use minijinja::{value::Value as MiniJinjaValue, Environment, UndefinedBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use simple_expand_tilde::expand_tilde;
use ssh2::Session;
use std::fs;
use std::io::prelude::*;
use std::net::TcpStream;
use std::path::Path;
use std::process::exit;
use std::process::Command;
use std::process::Stdio;

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

#[derive(Debug, Deserialize)]
struct Deployment {
    name: String,
    hosts: String,
    chdir: Option<String>,
    tasks: Vec<Task>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Task {
    name: String,
    shell: Option<String>,
    command: Option<String>,
    register: Option<String>,
    debug: Option<Debug>,
    vars: Option<IndexMap<String, String>>,
    chdir: Option<String>,
    when: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Debug(IndexMap<String, String>);

#[derive(Debug, Deserialize, Serialize)]
struct Register {
    stdout: String,
    stderr: String,
    rc: i32,
}

fn from_json_filter(value: MiniJinjaValue) -> MiniJinjaValue {
    value
}

fn read_yaml<T>(filename: &str) -> Result<T, Box<dyn std::error::Error>>
where
    T: for<'de> Deserialize<'de>,
{
    let contents = fs::read_to_string(filename)?;
    let yaml_data: T = serde_yaml::from_str(&contents)?;
    Ok(yaml_data)
}

fn read_yaml_multi<T>(filename: &str) -> Result<Vec<T>, Box<dyn std::error::Error>>
where
    T: for<'de> Deserialize<'de>,
{
    let contents = fs::read_to_string(filename)?;
    let mut results = Vec::new();

    for document in serde_yaml::Deserializer::from_str(&contents) {
        let item = T::deserialize(document)?;
        results.push(item);
    }

    Ok(results)
}

fn setup_ssh_session(
    host: &str,
    port: u16,
    user: &str,
    password: Option<&str>,
    ssh_key_path: Option<&str>,
) -> Result<Session, Box<dyn std::error::Error>> {
    let tcp = TcpStream::connect((host, port))?;
    let mut session = Session::new()?;
    session.set_tcp_stream(tcp);
    session.handshake()?;

    if let Some(key_path) = ssh_key_path {
        let resolved_key_path = expand_tilde(key_path).ok_or("Failed to resolve home directory")?;
        session.userauth_pubkey_file(user, None, &resolved_key_path, None)?;
    } else if let Some(pwd) = password {
        session.userauth_password(user, pwd)?;
    } else {
        return Err("Either ssh_key_path or password must be provided".into());
    }

    if !session.authenticated() {
        return Err("Authentication failed".into());
    }

    Ok(session)
}

fn execute_task(
    session: &Session,
    command: &str,
    use_shell: bool,
    display_output: bool,
    chdir: Option<&str>,
) -> Result<(String, String, i32), Box<dyn std::error::Error>> {
    session.set_blocking(true);
    let mut channel = session.channel_session()?;

    if let Some(dir) = chdir {
        channel.exec(&format!(
            "cd {} && {}",
            dir,
            if use_shell {
                format!("sh -c \"{}\"", command)
            } else {
                command.to_string()
            }
        ))?;
    } else {
        if use_shell {
            channel.exec(&format!("sh -c \"{}\"", command))?;
        } else {
            channel.exec(command)?;
        }
    }

    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut stdout_buffer = [0; 1024];
    let mut stderr_buffer = [0; 1024];

    loop {
        match channel.read(&mut stdout_buffer) {
            Ok(read_bytes) => {
                if read_bytes > 0 {
                    let output = String::from_utf8_lossy(&stdout_buffer[..read_bytes]);
                    stdout.push_str(&output);
                    if display_output {
                        print!("{}", format!("{}", output).white());
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => (),
            Err(e) => return Err(e.into()),
        }

        match channel.stderr().read(&mut stderr_buffer) {
            Ok(read_bytes) => {
                if read_bytes > 0 {
                    let error_output = String::from_utf8_lossy(&stderr_buffer[..read_bytes]);
                    stderr.push_str(&error_output);
                    if display_output {
                        print!("{}", format!("{}", error_output).red());
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => (),
            Err(e) => return Err(e.into()),
        }

        if channel.eof() {
            break;
        }
    }

    channel.wait_close()?;
    let exit_status = channel.exit_status()?;

    Ok((stdout, stderr, exit_status))
}

fn execute_local_task(
    command: &str,
    use_shell: bool,
    display_output: bool,
    chdir: Option<&str>,
) -> Result<(String, String, i32), Box<dyn std::error::Error>> {
    let mut cmd = if use_shell {
        let mut shell_cmd = Command::new("sh");
        shell_cmd.arg("-c").arg(command);
        shell_cmd
    } else {
        let parts =
            shell_words::split(command).map_err(|e| format!("Failed to parse command: {}", e))?;
        let mut cmd = Command::new(&parts[0]);
        if parts.len() > 1 {
            cmd.args(&parts[1..]);
        }
        cmd
    };

    if let Some(dir) = chdir {
        cmd.current_dir(dir);
    }

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().ok_or("Failed to open stdout")?;
    let stderr = child.stderr.take().ok_or("Failed to open stderr")?;

    let mut stdout_str = String::new();
    let mut stderr_str = String::new();

    let stdout_reader = std::io::BufReader::new(stdout).lines();
    let stderr_reader = std::io::BufReader::new(stderr).lines();

    for line in stdout_reader {
        if let Ok(line) = line {
            if display_output {
                println!("{}", line.white());
            }
            if !stdout_str.is_empty() {
                stdout_str.push('\n');
            }
            stdout_str.push_str(&line);
        }
    }

    for line in stderr_reader {
        if let Ok(line) = line {
            if display_output {
                eprintln!("{}", line.red());
            }
            if !stderr_str.is_empty() {
                stderr_str.push('\n');
            }
            stderr_str.push_str(&line);
        }
    }

    let exit_status = child.wait()?.code().unwrap_or(-1);

    Ok((stdout_str, stderr_str, exit_status))
}

fn replace_placeholders(
    msg: &str,
    register_map: &IndexMap<String, Register>,
    vars: &IndexMap<String, Value>,
) -> String {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.add_filter("from_json", from_json_filter);
    let template = env.template_from_str(msg).unwrap();
    let mut context = IndexMap::new();

    for (key, value) in register_map {
        context.insert(key.clone(), serde_json::to_value(value).unwrap());
    }

    for (key, value) in vars {
        context.insert(key.clone(), value.clone());
    }

    // Debug print to verify context
    // println!("Context: {:?}", context);

    template.render(&context).unwrap_or_else(|err| {
        if let minijinja::ErrorKind::UndefinedError = err.kind() {
            eprintln!(
                "{}",
                format!(
                    "One or more of the variables are undefined in:\n\"{}\"",
                    msg
                )
                .red()
            );
            eprintln!("{}", format!("Available vars: {:#?}", context).red());
        } else {
            eprintln!("{}", format!("Error rendering template: {}", err).red());
        }

        exit(1);
    })
}

fn replace_placeholders_vars(
    msg: &str,
    register_map: &IndexMap<String, Register>,
    vars: &IndexMap<String, Value>,
) -> Value {
    let rendered_str = replace_placeholders(msg, register_map, vars);

    if msg.contains("from_json") {
        serde_json::from_str(&rendered_str).unwrap_or_else(|err| {
            eprintln!(
                "{}",
                format!("Error parsing JSON: {}:\n{}\nat {}", err, rendered_str, msg).red()
            );
            exit(1);
        })
    } else {
        Value::String(rendered_str)
    }
}

fn split_commands(input: &str) -> Vec<String> {
    let lines: Vec<&str> = input.lines().collect();
    let mut commands = Vec::new();
    let mut current_command = String::new();

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.ends_with('\\') {
            // Remove the trailing backslash and any whitespace before it
            let clean_line = trimmed.trim_end_matches('\\').trim_end();
            current_command.push_str(clean_line);
            current_command.push(' '); // Add space between continued lines
        } else {
            current_command.push_str(trimmed);
            commands.push(current_command.clone());
            current_command.clear();
        }
    }

    // Handle last command if it doesn't end with newline
    if !current_command.is_empty() {
        commands.push(current_command);
    }

    commands
}

fn handle_command_execution(
    is_localhost: bool,
    session: Option<&Session>,
    command: &str,
    use_shell: bool,
    display_output: bool,
    chdir: Option<&str>,
    register: Option<&String>,
    register_map: &mut IndexMap<String, Register>,
) -> Result<(), Box<dyn std::error::Error>> {
    let result = if is_localhost {
        execute_local_task(command, use_shell, display_output, chdir)
    } else {
        execute_task(session.unwrap(), command, use_shell, display_output, chdir)
    };

    match result {
        Ok((stdout, stderr, exit_status)) => {
            if exit_status != 0 {
                return Err(format!(
                    "Command execution failed with exit status: {}. Stopping further tasks.",
                    exit_status
                )
                .red()
                .into());
            }

            if let Some(register) = register {
                register_map.insert(
                    register.clone(),
                    Register {
                        stdout: stdout.clone(),
                        stderr: stderr.clone(),
                        rc: exit_status,
                    },
                );
                println!(
                    "{}",
                    format!("Registering output to: {}", register).yellow()
                );
            }
        }
        Err(e) => {
            return Err(format!(
                "Command execution failed with error: {}. Stopping further tasks.",
                e
            )
            .red()
            .into());
        }
    }

    Ok(())
}

fn process_commands(
    commands: Vec<String>,
    is_localhost: bool,
    session: Option<&Session>,
    use_shell: bool,
    task_chdir: Option<&str>,
    register: Option<&String>,
    register_map: &mut IndexMap<String, Register>,
    vars_map: &IndexMap<String, Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    for cmd in commands {
        let substituted_cmd = replace_placeholders(&cmd, register_map, vars_map);
        println!("{}", format!("> {}", substituted_cmd).magenta());

        let display_output = register.is_none();
        handle_command_execution(
            is_localhost,
            session,
            &substituted_cmd,
            use_shell,
            display_output,
            task_chdir,
            register,
            register_map,
        )?;
    }

    Ok(())
}

fn should_run_task(
    condition: &Option<String>,
    register_map: &IndexMap<String, Register>,
    vars_map: &IndexMap<String, Value>,
) -> bool {
    if let Some(cond) = condition {
        let template_str = format!("{{% if {} %}}true{{% else %}}false{{% endif %}}", cond);
        let rendered_cond = replace_placeholders(&template_str, register_map, vars_map);
        if rendered_cond == "false" {
            false
        } else {
            true
        }
    } else {
        true
    }
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
                .help("Set additional variables as key=value, JSON, or @file")
                .num_args(1),
        )
        .get_matches();

    let deploy_file = matches.get_one::<String>("deploy_file").unwrap();
    let extra_vars = matches.get_one::<String>("extra_vars").map(|s| s.as_str());

    let server_config: ServerConfig = read_yaml("servers.yml")?;
    let deployment_docs: Vec<Vec<Deployment>> = read_yaml_multi(deploy_file)?;
    let deployments = deployment_docs.into_iter().flatten().collect::<Vec<_>>();

    let mut register_map: IndexMap<String, Register> = IndexMap::new();
    let mut vars_map: IndexMap<String, Value> = IndexMap::new();

    if let Some(extra_vars) = extra_vars {
        if extra_vars.starts_with('@') {
            let extra_vars_file = &extra_vars[1..];
            let extra_vars_path = Path::new(extra_vars_file);
            if extra_vars_path.exists() {
                let yaml_vars: IndexMap<String, Value> = read_yaml(extra_vars_file)?;
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

    for dep in deployments {
        println!("{}", format!("Starting deployment: {}\n", dep.name).green());

        let hosts: Vec<&str> = dep.hosts.split(',').map(|s| s.trim()).collect();

        let hosts_len = hosts.len();

        for host in hosts {
            if hosts_len > 1 {
                println!("{}", format!("Processing host: {}\n", host).blue());
            }

            if let Some(target_host) = server_config.hosts.get(host) {
                let is_localhost = target_host.host == "localhost";
                let session = if !is_localhost {
                    let port = target_host.port.ok_or("Missing port for remote host")?;
                    let user = target_host
                        .user
                        .as_deref()
                        .ok_or("Missing user for remote host")?;
                    let password = target_host.password.as_deref();
                    let ssh_key_path = target_host.ssh_key_path.as_deref();

                    Some(setup_ssh_session(
                        &target_host.host,
                        port,
                        user,
                        password,
                        ssh_key_path,
                    )?)
                } else {
                    None
                };

                for task in &dep.tasks {
                    if !should_run_task(&task.when, &register_map, &vars_map) {
                        println!("{}", format!("Skipping task: {}\n", task.name).yellow());
                        continue;
                    }

                    println!("{}", format!("Executing task: {}", task.name).cyan());

                    let task_chdir = task.chdir.as_deref().or(dep.chdir.as_deref()); // Use task-level chdir if present, otherwise use top-level chdir

                    if let Some(vars) = &task.vars {
                        for (key, value) in vars {
                            let evaluated_value =
                                replace_placeholders_vars(&value, &register_map, &vars_map);
                            vars_map.insert(key.clone(), evaluated_value);
                        }
                    }

                    // Debug print to verify vars_map
                    // println!("Vars map: {:?}", vars_map);

                    if let Some(debug) = &task.debug {
                        println!("{}", "Debug:".blue());
                        for (key, msg) in debug.0.iter() {
                            println!("{}", format!("{}:", key).blue());
                            let debug_msg = replace_placeholders(msg, &register_map, &vars_map);
                            println!("{}", format!("{}", debug_msg).blue());
                        }
                    }

                    if let Some(shell_command) = &task.shell {
                        let commands = split_commands(shell_command);
                        process_commands(
                            commands,
                            is_localhost,
                            session.as_ref(),
                            true,
                            task_chdir,
                            task.register.as_ref(),
                            &mut register_map,
                            &vars_map,
                        )?;
                    }

                    if let Some(command) = &task.command {
                        let commands = split_commands(command);
                        process_commands(
                            commands,
                            is_localhost,
                            session.as_ref(),
                            false,
                            task_chdir,
                            task.register.as_ref(),
                            &mut register_map,
                            &vars_map,
                        )?;
                    }

                    println!();
                }
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

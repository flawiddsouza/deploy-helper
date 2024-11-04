use colored::*;
use minijinja::{value::Value as MiniJinjaValue, Environment};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ssh2::Session;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::prelude::*;
use std::net::TcpStream;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;

#[derive(Debug, Deserialize)]
struct ServerConfig {
    hosts: HashMap<String, TargetHost>,
}

#[derive(Debug, Deserialize)]
struct TargetHost {
    host: String,
    port: Option<u16>,
    user: Option<String>,
    password: Option<String>,
    ssh_key: Option<String>,
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
    vars: Option<HashMap<String, String>>,
    chdir: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Debug {
    msg: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Register {
    stdout: String,
    stderr: String,
    rc: i32,
}

// Custom from_json filter
fn from_json_filter(value: MiniJinjaValue) -> Result<MiniJinjaValue, minijinja::Error> {
    let json_str = value.as_str().ok_or_else(|| {
        minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, "Expected a string")
    })?;
    let json_value: Value = serde_json::from_str(json_str).map_err(|e| {
        minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
    })?;
    Ok(MiniJinjaValue::from_serialize(&json_value))
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
        session.userauth_pubkey_file(user, None, Path::new(key_path), None)?;
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
    channel.read_to_string(&mut stdout)?;
    channel.stderr().read_to_string(&mut stderr)?;
    let exit_status = channel.exit_status()?;
    channel.wait_close()?;

    if display_output {
        if !stdout.is_empty() {
            println!("{}", format!("Output:\n{}", stdout).white());
        }
        if !stderr.is_empty() {
            println!("{}", format!("Error Output:\n{}", stderr).red());
        }
    }

    Ok((stdout, stderr, exit_status))
}

fn execute_local_task(
    command: &str,
    use_shell: bool,
    display_output: bool,
    chdir: Option<&str>,
) -> Result<(String, String, i32), Box<dyn std::error::Error>> {
    let output = if use_shell {
        if let Some(dir) = chdir {
            Command::new("sh")
                .arg("-c")
                .arg(format!("cd {} && {}", dir, command))
                .output()?
        } else {
            Command::new("sh").arg("-c").arg(command).output()?
        }
    } else {
        // Split the command into program and arguments
        let mut parts =
            shell_words::split(command).map_err(|e| format!("Failed to parse command: {}", e))?;
        if parts.is_empty() {
            return Err("Empty command provided".into());
        }
        let program = parts.remove(0);
        let mut cmd = Command::new(program);
        if let Some(dir) = chdir {
            cmd.current_dir(dir);
        }
        cmd.args(parts)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_status = output.status.code().unwrap_or(-1);

    if display_output {
        if !stdout.is_empty() {
            println!("{}", format!("Output:\n{}", stdout).white());
        }
        if !stderr.is_empty() {
            println!("{}", format!("Error Output:\n{}", stderr).red());
        }
    }

    if exit_status != 0 {
        return Err(format!(
            "Command '{}' failed with exit status: {}",
            command, exit_status
        )
        .into());
    }

    Ok((stdout, stderr, exit_status))
}

fn replace_placeholders(
    msg: &str,
    register_map: &HashMap<String, Register>,
    vars: &HashMap<String, Value>,
) -> String {
    let mut env = Environment::new();
    env.add_filter("from_json", from_json_filter); // Register the custom filter
    let template = env.template_from_str(msg).unwrap();
    let mut context = HashMap::new();

    for (key, value) in register_map {
        context.insert(key.clone(), serde_json::to_value(value).unwrap());
    }

    for (key, value) in vars {
        context.insert(key.clone(), value.clone());
    }

    // Debug print to verify context
    // println!("Context: {:?}", context);

    template.render(&context).unwrap()
}

fn replace_placeholders_vars(
    msg: &str,
    register_map: &HashMap<String, Register>,
    vars: &HashMap<String, Value>,
) -> Value {
    let rendered_str = replace_placeholders(msg, register_map, vars);

    if msg.contains("from_json") {
        serde_json::from_str(&rendered_str).unwrap()
    } else {
        Value::String(rendered_str)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        return Err("Usage: deploy-helper <deploy.yml>".into());
    }
    let deploy_file = &args[1];

    let server_config: ServerConfig = read_yaml("servers.yml")?;
    let deployment_docs: Vec<Vec<Deployment>> = read_yaml_multi(deploy_file)?;
    let deployments = deployment_docs.into_iter().flatten().collect::<Vec<_>>();

    let mut register_map: HashMap<String, Register> = HashMap::new();
    let mut vars_map: HashMap<String, Value> = HashMap::new(); // Add vars_map to store variables

    for dep in deployments {
        println!("{}", format!("Starting deployment: {}\n", dep.name).green()); // Print deployment name in green

        if let Some(target_host) = server_config.hosts.get(&dep.hosts) {
            let is_localhost = target_host.host == "localhost";
            let session = if !is_localhost {
                let port = target_host.port.ok_or("Missing port for remote host")?;
                let user = target_host
                    .user
                    .as_deref()
                    .ok_or("Missing user for remote host")?;
                let password = target_host.password.as_deref();
                let ssh_key = target_host.ssh_key.as_deref();

                Some(setup_ssh_session(
                    &target_host.host,
                    port,
                    user,
                    password,
                    ssh_key,
                )?)
            } else {
                None
            };

            for task in dep.tasks {
                println!("{}", format!("Executing task: {}", task.name).cyan()); // Print task name in cyan

                let task_chdir = task.chdir.as_deref().or(dep.chdir.as_deref()); // Use task-level chdir if present, otherwise use top-level chdir

                if let Some(shell_command) = task.shell {
                    // Substitute Jinja variables in shell_command
                    let substituted_shell_command =
                        replace_placeholders(&shell_command, &register_map, &vars_map);
                    println!("{}", format!("> {}", substituted_shell_command).magenta());

                    let display_output = task.register.is_none();
                    let result = if is_localhost {
                        execute_local_task(
                            &substituted_shell_command,
                            true,
                            display_output,
                            task_chdir,
                        )
                    } else {
                        execute_task(
                            session.as_ref().unwrap(),
                            &substituted_shell_command,
                            true,
                            display_output,
                            task_chdir,
                        )
                    };

                    match result {
                        Ok((stdout, stderr, exit_status)) => {
                            if exit_status != 0 {
                                return Err(format!("Command execution failed with exit status: {}. Stopping further tasks.", exit_status).red().into());
                            }

                            // Store the output in the register map if register is present
                            if let Some(register) = &task.register {
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
                                ); // Print register message in yellow
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
                }

                if let Some(command) = task.command {
                    // Substitute Jinja variables in command
                    let substituted_command =
                        replace_placeholders(&command, &register_map, &vars_map);
                    println!("{}", format!("> {}", substituted_command).magenta());

                    let display_output = task.register.is_none();
                    let result = if is_localhost {
                        execute_local_task(&substituted_command, false, display_output, task_chdir)
                    } else {
                        execute_task(
                            session.as_ref().unwrap(),
                            &substituted_command,
                            false,
                            display_output,
                            task_chdir,
                        )
                    };

                    match result {
                        Ok((stdout, stderr, exit_status)) => {
                            if exit_status != 0 {
                                return Err(format!("Command execution failed with exit status: {}. Stopping further tasks.", exit_status).red().into());
                            }

                            if let Some(register) = &task.register {
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
                }

                if let Some(vars) = &task.vars {
                    for (key, value) in vars {
                        let evaluated_value =
                            replace_placeholders_vars(&value, &register_map, &vars_map);
                        vars_map.insert(key.clone(), evaluated_value);
                    }
                }

                // Debug print to verify vars_map
                // println!("Vars map: {:?}", vars_map);

                // Use the debug field if present
                if let Some(debug) = &task.debug {
                    // Replace placeholders with registered values
                    let debug_msg = replace_placeholders(&debug.msg, &register_map, &vars_map);
                    print!("{}", format!("Debug:\n{}", debug_msg).blue()); // Print debug message in blue
                }

                println!(); // Add a new line after each task execution
            }
        } else {
            eprintln!(
                "{}",
                format!("No server config found for host: {}", dep.hosts).red()
            ); // Print error message in red
        }
    }

    Ok(())
}

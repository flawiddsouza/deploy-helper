use colored::*;
use minijinja::Environment;
use serde::Deserialize;
use ssh2::Session;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::prelude::*;
use std::net::TcpStream;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct ServerConfig {
    hosts: HashMap<String, TargetHost>,
}

#[derive(Debug, Deserialize)]
struct TargetHost {
    host: String,
    port: u16,
    user: String,
    ssh_key: String,
}

#[derive(Debug, Deserialize)]
struct Deployment {
    name: String,
    hosts: String,
    tasks: Vec<Task>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Task {
    name: String,
    shell: Option<String>,
    register: Option<String>,
    debug: Option<Debug>,
}

#[derive(Debug, Deserialize)]
struct Debug {
    msg: String,
}

#[derive(Debug, Deserialize)]
struct Register {
    stdout: String,
    stderr: String,
    rc: i32,
}

fn read_yaml<T>(filename: &str) -> Result<T, Box<dyn std::error::Error>>
where
    T: for<'de> Deserialize<'de>,
{
    let contents = fs::read_to_string(filename)?;
    let yaml_data: T = serde_yaml::from_str(&contents)?;
    Ok(yaml_data)
}

fn setup_ssh_session(
    host: &str,
    port: u16,
    user: &str,
    ssh_key_path: &str,
) -> Result<Session, Box<dyn std::error::Error>> {
    let tcp = TcpStream::connect((host, port))?;
    let mut session = Session::new()?;
    session.set_tcp_stream(tcp);
    session.handshake()?;
    session.userauth_pubkey_file(user, None, Path::new(ssh_key_path), None)?;

    if !session.authenticated() {
        return Err("Authentication failed".into());
    }

    Ok(session)
}

fn execute_task(
    session: &Session,
    command: &str,
    display_output: bool,
) -> Result<(String, String, i32), Box<dyn std::error::Error>> {
    let mut channel = session.channel_session()?;
    channel.exec(command)?;
    let mut stdout = String::new();
    let mut stderr = String::new();
    channel.read_to_string(&mut stdout)?;
    channel.stderr().read_to_string(&mut stderr)?;
    let exit_status = channel.exit_status()?;
    channel.wait_close()?;

    if display_output {
        println!("{}", format!("Output:\n{}", stdout).white()); // Print stdout in white if display_output is true
    }

    Ok((stdout, stderr, exit_status))
}

fn replace_placeholders(msg: &str, register_map: &HashMap<String, Register>) -> String {
    let env = Environment::new();
    let template = env.template_from_str(msg).unwrap();
    let context: HashMap<String, HashMap<&str, String>> = register_map
        .iter()
        .map(|(k, v)| {
            let mut map = HashMap::new();
            map.insert("stdout", v.stdout.clone());
            map.insert("stderr", v.stderr.clone());
            map.insert("rc", v.rc.to_string());
            (k.clone(), map)
        })
        .collect();
    template.render(&context).unwrap()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        return Err("Usage: deploy-helper <deploy.yml>".into());
    }
    let deploy_file = &args[1];

    let server_config: ServerConfig = read_yaml("servers.yml")?;
    let deployment: Vec<Deployment> = read_yaml(deploy_file)?;
    let mut register_map: HashMap<String, Register> = HashMap::new(); // Change the type of register_map

    for dep in deployment {
        println!("{}", format!("Starting deployment: {}\n", dep.name).green()); // Print deployment name in green

        if let Some(target_host) = server_config.hosts.get(&dep.hosts) {
            let session = setup_ssh_session(
                &target_host.host,
                target_host.port,
                &target_host.user,
                &target_host.ssh_key,
            )?;

            for task in dep.tasks {
                println!("{}", format!("Executing task: {}", task.name).cyan()); // Print task name in cyan

                if let Some(shell_command) = task.shell {
                    println!("{}", format!("> {}", shell_command).magenta()); // Print command being run in magenta

                    let display_output = task.register.is_none();
                    match execute_task(&session, &shell_command, display_output) {
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

                // Use the debug field if present
                if let Some(debug) = &task.debug {
                    // Replace placeholders with registered values
                    let debug_msg = replace_placeholders(&debug.msg, &register_map);
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

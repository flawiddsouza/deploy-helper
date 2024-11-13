use colored::Colorize;
use indexmap::IndexMap;
use minijinja::{value::Value as MiniJinjaValue, Environment, UndefinedBehavior};
use serde::Deserialize;
use serde_json::Value;
use simple_expand_tilde::expand_tilde;
use ssh2::Session;
use std::fs;
use std::io::prelude::*;
use std::net::TcpStream;
use std::process::{exit, Command, Stdio};

pub fn replace_placeholders(msg: &str, vars: &IndexMap<String, Value>) -> String {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.add_filter("from_json", from_json_filter);
    let template = env.template_from_str(msg).unwrap();
    let mut context = IndexMap::new();

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

pub fn replace_placeholders_vars(msg: &str, vars: &IndexMap<String, Value>) -> Value {
    let rendered_str = replace_placeholders(msg, vars);

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

pub fn split_commands(input: &str) -> Vec<String> {
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

pub fn from_json_filter(value: MiniJinjaValue) -> MiniJinjaValue {
    value
}

pub fn read_yaml<T>(filename: &str) -> Result<T, Box<dyn std::error::Error>>
where
    T: for<'de> Deserialize<'de>,
{
    let contents = fs::read_to_string(filename)?;
    let yaml_data: T = serde_yaml::from_str(&contents)?;
    Ok(yaml_data)
}

pub fn read_yaml_multi<T>(filename: &str) -> Result<Vec<T>, Box<dyn std::error::Error>>
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

pub fn setup_ssh_session(
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

pub fn execute_ssh_command(
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

pub fn execute_local_command(
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

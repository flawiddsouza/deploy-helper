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

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn wrap_become_command(command: &str, method: &str, password: Option<&str>) -> String {
    if method == "su" {
        if let Some(pw) = password {
            format!(
                "printf '%s\\n' {} | su -c {}",
                shell_escape(pw),
                shell_escape(command)
            )
        } else {
            format!("su -c {}", shell_escape(command))
        }
    } else if let Some(pw) = password {
        format!(
            "printf '%s\\n' {} | {} -S -p '' sh -c {}",
            shell_escape(pw),
            method,
            shell_escape(command)
        )
    } else {
        format!("{} sh -c {}", method, shell_escape(command))
    }
}

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

fn heredoc_delimiter(line: &str) -> Option<String> {
    let pos = line.find("<<")?;
    let after = line[pos + 2..].trim_start();
    let after = after.strip_prefix('-').unwrap_or(after).trim_start();
    let raw = if let Some(rest) = after.strip_prefix('\'') {
        rest.split('\'').next()?
    } else if let Some(rest) = after.strip_prefix('"') {
        rest.split('"').next()?
    } else {
        after.split(|c: char| c.is_whitespace()).next()?
    };
    if raw.is_empty() { None } else { Some(raw.to_string()) }
}

pub fn split_commands(input: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut current_command = String::new();
    let mut heredoc_end: Option<String> = None;

    for line in input.lines() {
        if let Some(ref delimiter) = heredoc_end {
            current_command.push('\n');
            current_command.push_str(line);
            if line.trim() == delimiter.as_str() {
                heredoc_end = None;
                commands.push(current_command.clone());
                current_command.clear();
            }
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.ends_with('\\') {
            let clean_line = trimmed.trim_end_matches('\\').trim_end();
            current_command.push_str(clean_line);
            current_command.push(' ');
        } else {
            current_command.push_str(trimmed);
            if let Some(delim) = heredoc_delimiter(trimmed) {
                heredoc_end = Some(delim);
            } else {
                commands.push(current_command.clone());
                current_command.clear();
            }
        }
    }

    if !current_command.is_empty() {
        commands.push(current_command);
    }

    commands
}

pub fn from_json_filter(value: MiniJinjaValue) -> MiniJinjaValue {
    value
}

fn annotate_yaml_error(filename: &str, contents: &str, err: serde_yaml::Error) -> String {
    let msg = err.to_string();
    if !msg.contains("invalid type: map, expected a string") {
        return format!("{}: {}", filename, msg);
    }
    let Some(loc) = err.location() else {
        return format!("{}: {}", filename, msg);
    };
    let line_no = loc.line();
    let Some(line) = contents.lines().nth(line_no.saturating_sub(1)) else {
        return format!("{}: {}", filename, msg);
    };
    if !line.contains("{{") {
        return format!("{}: {}", filename, msg);
    }
    format!(
        "{}: line {} has an unquoted {{{{ ... }}}} value:\n    {}\n  YAML reads a leading {{ as the start of an inline object, so {{{{ var }}}} gets parsed as a nested object instead of text.\n  Wrap it in quotes so YAML treats it as a string, e.g. \"{{{{ var }}}}\" or \"{{{{ var }}}}/path\".",
        filename,
        line_no,
        line.trim_end(),
    )
}

fn read_file_or_exit(filename: &str) -> String {
    fs::read_to_string(filename).unwrap_or_else(|e| {
        eprintln!("{}", format!("Failed to read {}: {}", filename, e).red());
        exit(1);
    })
}

pub fn read_yaml<T>(filename: &str) -> T
where
    T: for<'de> Deserialize<'de>,
{
    let contents = read_file_or_exit(filename);
    serde_yaml::from_str(&contents).unwrap_or_else(|e| {
        eprintln!("{}", annotate_yaml_error(filename, &contents, e).red());
        exit(1);
    })
}

pub fn read_yaml_multi<T>(filename: &str) -> Vec<T>
where
    T: for<'de> Deserialize<'de>,
{
    let contents = read_file_or_exit(filename);
    let mut results = Vec::new();

    for document in serde_yaml::Deserializer::from_str(&contents) {
        let item = T::deserialize(document).unwrap_or_else(|e| {
            eprintln!("{}", annotate_yaml_error(filename, &contents, e).red());
            exit(1);
        });
        results.push(item);
    }

    results
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
    login_shell: bool,
) -> Result<(String, String, i32), Box<dyn std::error::Error>> {
    session.set_blocking(true);
    let mut channel = session.channel_session()?;

    // Use $SHELL -l -i so login files (.zprofile/.bash_profile) and interactive
    // files (.zshrc/.bashrc) are both sourced — required for PATH entries added
    // by tools like bun/nvm that only appear in .bashrc/.zshrc.
    let final_cmd = if login_shell {
        let base = if let Some(dir) = chdir {
            format!("cd {} && {}", dir, command)
        } else {
            command.to_string()
        };
        let sh_arg = format!("exec \"$SHELL\" -l -i -c {}", shell_escape(&base));
        format!("sh -c {}", shell_escape(&sh_arg))
    } else if let Some(dir) = chdir {
        let base = format!("cd {} && {}", dir, command);
        if use_shell {
            format!("sh -c {}", shell_escape(&base))
        } else {
            base
        }
    } else if use_shell {
        format!("sh -c {}", shell_escape(command))
    } else {
        command.to_string()
    };

    channel.exec(&final_cmd)?;

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
                        print!("{}", output.white());
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
                        print!("{}", error_output.red());
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

    // BufReader::lines() used in local execution strips trailing newlines;
    // match that behaviour here so registered output is consistent.
    let stdout = stdout.trim_end_matches(['\n', '\r']).to_string();
    let stderr = stderr.trim_end_matches(['\n', '\r']).to_string();

    Ok((stdout, stderr, exit_status))
}

pub fn execute_local_command(
    command: &str,
    use_shell: bool,
    display_output: bool,
    chdir: Option<&str>,
    login_shell: bool,
) -> Result<(String, String, i32), Box<dyn std::error::Error>> {
    let mut cmd = if login_shell && !cfg!(windows) {
        let sh_arg = format!("exec \"$SHELL\" -l -i -c {}", shell_escape(command));
        let mut c = Command::new("sh");
        c.arg("-c").arg(sh_arg);
        c
    } else if use_shell {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
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

#[cfg(test)]
mod tests {
    use super::*;

    // shell_escape

    #[test]
    fn test_shell_escape_simple() {
        assert_eq!(shell_escape("hello"), "'hello'");
    }

    #[test]
    fn test_shell_escape_with_spaces() {
        assert_eq!(shell_escape("hello world"), "'hello world'");
    }

    #[test]
    fn test_shell_escape_with_single_quote() {
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn test_shell_escape_with_special_chars() {
        assert_eq!(shell_escape("a && b | c"), "'a && b | c'");
    }

    // heredoc_delimiter

    #[test]
    fn test_heredoc_delimiter_single_quoted() {
        assert_eq!(heredoc_delimiter("cat << 'EOF' > file"), Some("EOF".to_string()));
    }

    #[test]
    fn test_heredoc_delimiter_double_quoted() {
        assert_eq!(heredoc_delimiter("cat << \"EOF\""), Some("EOF".to_string()));
    }

    #[test]
    fn test_heredoc_delimiter_unquoted() {
        assert_eq!(heredoc_delimiter("cat << EOF"), Some("EOF".to_string()));
    }

    #[test]
    fn test_heredoc_delimiter_dash() {
        assert_eq!(heredoc_delimiter("cat <<- 'EOF'"), Some("EOF".to_string()));
    }

    #[test]
    fn test_heredoc_delimiter_none() {
        assert_eq!(heredoc_delimiter("echo hello"), None);
    }

    // split_commands

    #[test]
    fn test_split_commands_single() {
        assert_eq!(split_commands("echo hello"), vec!["echo hello"]);
    }

    #[test]
    fn test_split_commands_multiple() {
        let input = "echo one\necho two\necho three";
        assert_eq!(split_commands(input), vec!["echo one", "echo two", "echo three"]);
    }

    #[test]
    fn test_split_commands_skips_empty_lines() {
        let input = "echo one\n\necho two";
        assert_eq!(split_commands(input), vec!["echo one", "echo two"]);
    }

    #[test]
    fn test_split_commands_line_continuation() {
        let input = "echo \\\none \\\ntwo";
        assert_eq!(split_commands(input), vec!["echo one two"]);
    }

    #[test]
    fn test_split_commands_heredoc_single_quoted() {
        let input = "cat << 'EOF' > /tmp/file\nline one\nline two\nEOF";
        assert_eq!(split_commands(input), vec!["cat << 'EOF' > /tmp/file\nline one\nline two\nEOF"]);
    }

    #[test]
    fn test_split_commands_heredoc_unquoted() {
        let input = "cat << EOF\ncontent\nEOF";
        assert_eq!(split_commands(input), vec!["cat << EOF\ncontent\nEOF"]);
    }

    #[test]
    fn test_split_commands_heredoc_then_command() {
        let input = "cat << 'EOF' > /tmp/file\ncontent\nEOF\necho done";
        assert_eq!(
            split_commands(input),
            vec!["cat << 'EOF' > /tmp/file\ncontent\nEOF", "echo done"]
        );
    }

    #[test]
    fn test_split_commands_heredoc_preserves_indentation() {
        let input = "cat << 'EOF' > /tmp/file\n    indented\n        more\nEOF";
        assert_eq!(
            split_commands(input),
            vec!["cat << 'EOF' > /tmp/file\n    indented\n        more\nEOF"]
        );
    }

    // wrap_become_command

    #[test]
    fn test_wrap_become_sudo_with_password() {
        let result = wrap_become_command("nginx -t && systemctl reload nginx", "sudo", Some("secret"));
        assert_eq!(
            result,
            "printf '%s\\n' 'secret' | sudo -S -p '' sh -c 'nginx -t && systemctl reload nginx'"
        );
    }

    #[test]
    fn test_wrap_become_sudo_nopasswd() {
        let result = wrap_become_command("nginx -t", "sudo", None);
        assert_eq!(result, "sudo sh -c 'nginx -t'");
    }

    #[test]
    fn test_wrap_become_doas_nopasswd() {
        let result = wrap_become_command("nginx -t", "doas", None);
        assert_eq!(result, "doas sh -c 'nginx -t'");
    }

    #[test]
    fn test_wrap_become_password_with_special_chars() {
        let result = wrap_become_command("id", "sudo", Some("p@ss'word"));
        assert_eq!(result, "printf '%s\\n' 'p@ss'\\''word' | sudo -S -p '' sh -c 'id'");
    }

    #[test]
    fn test_wrap_become_su_with_password() {
        let result = wrap_become_command("nginx -t", "su", Some("secret"));
        assert_eq!(result, "printf '%s\\n' 'secret' | su -c 'nginx -t'");
    }

    #[test]
    fn test_wrap_become_su_nopasswd() {
        let result = wrap_become_command("nginx -t", "su", None);
        assert_eq!(result, "su -c 'nginx -t'");
    }

    // annotate_yaml_error

    #[test]
    fn test_annotate_yaml_error_unquoted_template() {
        let contents = "- name: x\n  chdir: {{ app_path }}\n";
        #[derive(serde::Deserialize, Debug)]
        struct S {
            #[allow(dead_code)]
            name: String,
            #[allow(dead_code)]
            chdir: String,
        }
        let err = serde_yaml::from_str::<Vec<S>>(contents).unwrap_err();
        let out = annotate_yaml_error("setup.yml", contents, err);
        assert!(out.contains("unquoted"), "missing plain-language hint in: {}", out);
        assert!(out.contains("line 2"), "missing line number in: {}", out);
        assert!(out.contains("chdir: {{ app_path }}"), "missing source line in: {}", out);
        assert!(!out.contains("invalid type: map"), "leaks raw serde_yaml jargon: {}", out);
    }

    #[test]
    fn test_annotate_yaml_error_passthrough_when_not_map_error() {
        let contents = "- name: x\n  chdir: [invalid\n";
        let err = serde_yaml::from_str::<serde_yaml::Value>(contents).unwrap_err();
        let out = annotate_yaml_error("setup.yml", contents, err);
        assert!(!out.contains("unquoted"), "should not add hint: {}", out);
    }

    #[test]
    fn test_annotate_yaml_error_no_hint_when_line_not_templated() {
        let contents = "- name: x\n  chdir: {key: val}\n";
        #[derive(serde::Deserialize, Debug)]
        struct S {
            #[allow(dead_code)]
            name: String,
            #[allow(dead_code)]
            chdir: String,
        }
        let err = serde_yaml::from_str::<Vec<S>>(contents).unwrap_err();
        let out = annotate_yaml_error("setup.yml", contents, err);
        assert!(!out.contains("unquoted"), "should not add hint: {}", out);
    }
}

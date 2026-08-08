use colored::Colorize;
use indexmap::IndexMap;
use serde_json::Value;
use ssh2::Session;

use crate::common::{EnvFileSecretsProvider, EnvFileSpec, Register};
use crate::utils;

// Merge dotenv files by key while preserving the defaults file's comments and
// ordering. values and secrets may replace defaults, but may not define the
// same key as each other.
const MERGE_AWK: &str = r#"
function fail(message) {
    print message > "/dev/stderr"
    failed = 1
    exit 1
}

function key_of(line, key) {
    if (line ~ /^[[:space:]]*$/ || line ~ /^[[:space:]]*#/) return ""
    if (line !~ /^[A-Za-z_][A-Za-z0-9_]*=/) {
        fail("invalid dotenv line in " FILENAME " at line " FNR)
    }
    key = line
    sub(/=.*/, "", key)
    return key
}

function apply_value(key, line) {
    if (key in position) {
        output[position[key]] = line
    } else {
        position[key] = ++output_count
        output[output_count] = line
    }
}

FILENAME == ARGV[1] {
    sub(/\r$/, "")
    key = key_of($0)
    if (key != "") {
        if (key in defaults_seen) fail("duplicate key in defaults: " key)
        defaults_seen[key] = 1
        position[key] = ++output_count
        output[output_count] = $0
    } else {
        output[++output_count] = $0
    }
    next
}

FILENAME == ARGV[2] {
    sub(/\r$/, "")
    key = key_of($0)
    if (key == "") next
    if (key in values_seen) fail("duplicate key in values: " key)
    values_seen[key] = 1
    apply_value(key, $0)
    next
}

FILENAME == ARGV[3] {
    sub(/\r$/, "")
    key = key_of($0)
    if (key == "") next
    if (key in secrets_seen) fail("duplicate key in secrets: " key)
    if (key in values_seen) fail("key is defined in both values and secrets: " key)
    secrets_seen[key] = 1
    secrets_count++
    apply_value(key, $0)
    next
}

END {
    if (!failed && require_secrets && secrets_count == 0) {
        fail("decrypted secrets contain no dotenv entries")
    }
    if (!failed) {
        for (i = 1; i <= output_count; i++) print output[i]
    }
}
"#;

// values: entries are written without quotes so the generated file keeps the
// same plain KEY=value form as the existing workflows. Restrict them to a
// portable scalar subset rather than guessing how each dotenv consumer quotes
// spaces, comments, interpolation, or shell metacharacters.
fn env_file_value_is_safe(value: &str) -> bool {
    value.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                '_' | '-' | '.' | '/' | ':' | '@' | '%' | '+' | ',' | '?' | '='
            )
    })
}

pub fn process(
    task_name: &str,
    spec: &EnvFileSpec,
    is_localhost: bool,
    session: Option<&Session>,
    chdir: Option<&str>,
    vars_map: &mut IndexMap<String, Value>,
    become_enabled: bool,
    become_method: &str,
    become_password: Option<&str>,
    register: Option<&String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let defaults = utils::replace_placeholders(&spec.defaults, vars_map);
    let dest = utils::replace_placeholders(&spec.dest, vars_map);
    let mode = utils::replace_placeholders(&spec.mode, vars_map);
    utils::validate_mode(&mode).map_err(|e| format!("Task '{}': {}", task_name, e))?;

    let values = utils::render_env_values(&spec.values, vars_map)
        .map_err(|e| format!("Task '{}': {}", task_name, e))?;
    for (key, value) in &values {
        if !env_file_value_is_safe(value) {
            return Err(format!(
                "Task '{}': env_file value '{}' is not a safe unquoted dotenv scalar; put complex values in defaults or secrets",
                task_name, key
            )
            .into());
        }
    }

    let secrets_src = spec
        .secrets
        .as_ref()
        .map(|secrets| utils::replace_placeholders(&secrets.src, vars_map));

    let dest_tmp_template = format!("{}.deploy-helper-tmp.XXXXXX", dest);
    let values_tmp_template = format!("{}.deploy-helper-values.XXXXXX", dest);
    let secrets_tmp_template = format!("{}.deploy-helper-secrets.XXXXXX", dest);

    let mut command = format!(
        "set -eu\numask 077\ndest_tmp=\nvalues_tmp=\nsecrets_tmp=\ncleanup() {{\n  if [ -n \"$dest_tmp\" ]; then rm -f \"$dest_tmp\"; fi\n  if [ -n \"$values_tmp\" ]; then rm -f \"$values_tmp\"; fi\n  if [ -n \"$secrets_tmp\" ]; then rm -f \"$secrets_tmp\"; fi\n}}\ntrap cleanup EXIT\ntrap 'exit 1' HUP INT TERM\ndest_tmp=$(mktemp {dest_tmp})\nvalues_tmp=$(mktemp {values_tmp})\nsecrets_tmp=$(mktemp {secrets_tmp})\n: > \"$values_tmp\"\n",
        dest_tmp = utils::shell_escape(&dest_tmp_template),
        values_tmp = utils::shell_escape(&values_tmp_template),
        secrets_tmp = utils::shell_escape(&secrets_tmp_template),
    );

    for (key, value) in &values {
        command.push_str(&format!(
            "printf '%s=%s\\n' {} {} >> \"$values_tmp\"\n",
            utils::shell_escape(key),
            utils::shell_escape(value)
        ));
    }

    if let (Some(secrets), Some(src)) = (&spec.secrets, &secrets_src) {
        match secrets.provider {
            EnvFileSecretsProvider::Sops => command.push_str(&format!(
                "if ! SOPS_DISABLE_VERSION_CHECK=1 sops -d {} > \"$secrets_tmp\"; then\n  echo {} >&2\n  exit 1\nfi\n",
                utils::shell_escape(src),
                utils::shell_escape(&format!("sops decryption failed: {}", src))
            )),
        }
    } else {
        command.push_str(": > \"$secrets_tmp\"\n");
    }

    command.push_str(&format!(
        "awk -v require_secrets={} {} {} \"$values_tmp\" \"$secrets_tmp\" > \"$dest_tmp\"\nchmod {} \"$dest_tmp\"\nrm -f \"$values_tmp\" \"$secrets_tmp\"\nvalues_tmp=\nsecrets_tmp=\nmv -f \"$dest_tmp\" {}\ndest_tmp=\ntrap - EXIT HUP INT TERM\n",
        u8::from(spec.secrets.is_some()),
        utils::shell_escape(MERGE_AWK),
        utils::shell_escape(&defaults),
        mode,
        utils::shell_escape(&dest),
    ));

    let command = if let Some(dir) = chdir {
        format!("cd {} && {}", utils::shell_escape(dir), command)
    } else {
        command
    };

    let source_note = if spec.secrets.is_some() {
        "defaults, values, sops secrets"
    } else {
        "defaults and values"
    };
    println!(
        "{}",
        format!("> [env_file] {} ({}, mode {})", dest, source_note, mode).magenta()
    );

    let (out, stderr, code) = utils::run_shell_on_target(
        &command,
        is_localhost,
        session,
        become_enabled,
        become_method,
        become_password,
    )?;
    if code != 0 {
        let detail = if stderr.trim().is_empty() {
            out.trim()
        } else {
            stderr.trim()
        };
        return Err(format!("Task '{}': env_file failed: {}", task_name, detail).into());
    }

    if let Some(reg) = register {
        let value = serde_json::to_value(Register {
            stdout: String::new(),
            stderr: String::new(),
            rc: 0,
        })?;
        vars_map.insert(reg.clone(), value);
        println!("{}", format!("Registering output to: {}", reg).yellow());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::env_file_value_is_safe;

    #[test]
    fn env_file_value_accepts_plain_scalars() {
        for value in [
            "",
            "abc123",
            "sha256:abc-def",
            "https://example.test/path?ref=abc",
            "image@example.test:443/name,other",
        ] {
            assert!(env_file_value_is_safe(value), "should accept {value:?}");
        }
    }

    #[test]
    fn env_file_value_rejects_ambiguous_dotenv_syntax() {
        for value in [
            "has space",
            "value # comment",
            "$INTERPOLATED",
            "single'quote",
            "double\"quote",
            "command;next",
            "a&b",
            "line\nbreak",
        ] {
            assert!(!env_file_value_is_safe(value), "should reject {value:?}");
        }
    }
}

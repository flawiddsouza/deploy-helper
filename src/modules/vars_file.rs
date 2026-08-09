use crate::common::{VarsFileProvider, VarsFileSpec};
use crate::utils;
use indexmap::IndexMap;
use serde_json::Value;
use std::path::Path;
use std::process::Command;

pub fn load_all(
    specs: &[VarsFileSpec],
    deploy_file_dir: &Path,
    vars_map: &mut IndexMap<String, Value>,
    vars_overrides: &IndexMap<String, Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    vars_map.extend(vars_overrides.clone());

    for spec in specs {
        let src = utils::replace_placeholders(&spec.src, vars_map);
        let src_path = utils::resolve_src_path(deploy_file_dir, &src);
        let decrypted = match spec.provider {
            VarsFileProvider::Sops => decrypt_sops(&src_path)?,
        };
        let vars = parse_vars(&src, &decrypted)?;
        vars_map.extend(vars);
        vars_map.extend(vars_overrides.clone());
    }
    Ok(())
}

fn decrypt_sops(src_path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let output = Command::new("sops")
        .arg("-d")
        .arg(src_path)
        .env("SOPS_DISABLE_VERSION_CHECK", "1")
        .output()
        .map_err(|error| {
            format!(
                "vars_files: could not run sops for '{}': {}",
                src_path.display(),
                error
            )
        })?;

    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let status = output
            .status
            .code()
            .map(|code| format!("exit status {}", code))
            .unwrap_or_else(|| "terminated by signal".to_string());
        if detail.is_empty() {
            return Err(format!(
                "vars_files: sops decryption failed for '{}' ({})",
                src_path.display(),
                status
            )
            .into());
        }
        return Err(format!(
            "vars_files: sops decryption failed for '{}' ({}): {}",
            src_path.display(),
            status,
            detail
        )
        .into());
    }

    Ok(output.stdout)
}

fn parse_vars(
    src: &str,
    decrypted: &[u8],
) -> Result<IndexMap<String, Value>, Box<dyn std::error::Error>> {
    if decrypted.iter().all(u8::is_ascii_whitespace) {
        return Err(format!("vars_files: decrypted file '{}' is empty", src).into());
    }
    let contents = std::str::from_utf8(decrypted)
        .map_err(|_| format!("vars_files: decrypted file '{}' is not valid UTF-8", src))?;
    let vars: IndexMap<String, Value> = serde_yaml::from_str(contents)
        .map_err(|error| format!("vars_files: decrypted file '{}': {}", src, error))?;
    if vars.is_empty() {
        return Err(format!("vars_files: decrypted file '{}' is empty", src).into());
    }
    Ok(vars)
}

#[cfg(test)]
mod tests {
    use super::parse_vars;
    use serde_json::json;

    #[test]
    fn parses_yaml_mapping_with_structured_values() {
        let vars = parse_vars(
            "secrets.enc.yml",
            b"token: secret\nports: [80, 443]\nsettings:\n  enabled: true\n",
        )
        .unwrap();
        assert_eq!(vars["token"], json!("secret"));
        assert_eq!(vars["ports"], json!([80, 443]));
        assert_eq!(vars["settings"], json!({"enabled": true}));
    }

    #[test]
    fn rejects_empty_decrypted_content() {
        let err = parse_vars("empty.enc.yml", b" \r\n\t").unwrap_err();
        assert_eq!(
            err.to_string(),
            "vars_files: decrypted file 'empty.enc.yml' is empty"
        );
    }

    #[test]
    fn rejects_empty_yaml_mapping() {
        let err = parse_vars("empty.enc.yml", b"{}\n").unwrap_err();
        assert_eq!(
            err.to_string(),
            "vars_files: decrypted file 'empty.enc.yml' is empty"
        );
    }

    #[test]
    fn rejects_non_mapping_yaml() {
        let err = parse_vars("list.enc.yml", b"- one\n- two\n").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("decrypted file 'list.enc.yml'"));
        assert!(message.contains("expected a map"));
    }

    #[test]
    fn rejects_invalid_utf8_without_echoing_content() {
        let err = parse_vars("bad.enc.yml", &[0xff, 0xfe]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "vars_files: decrypted file 'bad.enc.yml' is not valid UTF-8"
        );
    }
}

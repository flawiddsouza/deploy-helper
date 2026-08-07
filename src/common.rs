use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct Debug(pub IndexMap<String, String>);

#[derive(Debug, Deserialize, Serialize)]
pub struct Register {
    pub stdout: String,
    pub stderr: String,
    pub rc: i32,
}

// `mode:` values must be quoted strings: unquoted YAML `0600` is parsed as the
// number 600, silently changing the permissions. Reject numbers with a hint.
fn de_mode<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct ModeVisitor;

    impl serde::de::Visitor<'_> for ModeVisitor {
        type Value = String;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a quoted octal string like \"0600\"")
        }

        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<String, E> {
            Err(E::custom(format!(
                "mode must be a quoted string like \"0600\" (unquoted it is read as the YAML number {}, which loses the octal meaning)",
                v
            )))
        }

        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<String, E> {
            self.visit_u64(v.max(0) as u64)
        }
    }

    deserializer.deserialize_any(ModeVisitor).map(Some)
}

#[derive(Debug, Deserialize)]
pub struct Task {
    pub name: String,
    pub shell: Option<String>,
    pub command: Option<String>,
    pub creates: Option<String>,
    pub removes: Option<String>,
    pub register: Option<String>,
    pub no_log: Option<bool>,
    pub debug: Option<Debug>,
    pub vars: Option<IndexMap<String, String>>,
    pub chdir: Option<String>,
    pub when: Option<String>,
    pub r#loop: Option<Vec<Value>>,
    pub include_tasks: Option<String>,
    pub login_shell: Option<bool>,
    pub shell_defaults: Option<String>,
    pub r#become: Option<bool>,
    pub become_method: Option<String>,
    pub template: Option<TemplateSpec>,
    pub copy: Option<CopySpec>,
    pub file: Option<FileSpec>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct TemplateSpec {
    pub src: String,
    pub dest: String,
    #[serde(default, deserialize_with = "de_mode")]
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CopySpec {
    pub src: Option<String>,
    pub content: Option<String>,
    pub dest: String,
    #[serde(default, deserialize_with = "de_mode")]
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FileSpec {
    pub path: String,
    pub state: String,
    #[serde(default, deserialize_with = "de_mode")]
    pub mode: Option<String>,
    pub owner: Option<String>,
    pub group: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_parses_tags_as_list() {
        let yaml = "name: Example\nshell: echo hi\ntags: [build, web]\n";
        let task: Task = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            task.tags,
            Some(vec!["build".to_string(), "web".to_string()])
        );
    }

    #[test]
    fn task_parses_tags_block_form() {
        let yaml = "name: Example\nshell: echo hi\ntags:\n  - build\n  - web\n";
        let task: Task = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            task.tags,
            Some(vec!["build".to_string(), "web".to_string()])
        );
    }

    #[test]
    fn task_without_tags_is_none() {
        let yaml = "name: Example\nshell: echo hi\n";
        let task: Task = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(task.tags, None);
    }

    #[test]
    fn copy_mode_parses_quoted_string() {
        let yaml = "src: a\ndest: b\nmode: \"0600\"\n";
        let spec: CopySpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.mode, Some("0600".to_string()));
    }

    #[test]
    fn copy_mode_absent_is_none() {
        let yaml = "src: a\ndest: b\n";
        let spec: CopySpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.mode, None);
    }

    // YAML keeps a leading-zero scalar like 0600 as a string, so it parses
    // fine even unquoted.
    #[test]
    fn copy_mode_accepts_unquoted_leading_zero() {
        let yaml = "src: a\ndest: b\nmode: 0600\n";
        let spec: CopySpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.mode, Some("0600".to_string()));
    }

    // Without the leading zero, 600 is a YAML number - reject it with a hint
    // instead of silently reading the wrong permissions.
    #[test]
    fn copy_mode_rejects_unquoted_number() {
        let yaml = "src: a\ndest: b\nmode: 600\n";
        let err = serde_yaml::from_str::<CopySpec>(yaml).unwrap_err();
        assert!(
            err.to_string().contains("quoted"),
            "error should hint at quoting the mode: {}",
            err
        );
    }

    #[test]
    fn template_mode_parses_quoted_string() {
        let yaml = "src: a\ndest: b\nmode: \"644\"\n";
        let spec: TemplateSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.mode, Some("644".to_string()));
    }

    #[test]
    fn file_spec_parses_directory_state() {
        let yaml = "path: /srv/app\nstate: directory\nmode: \"0700\"\nowner: app\ngroup: app\n";
        let spec: FileSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.path, "/srv/app");
        assert_eq!(spec.state, "directory");
        assert_eq!(spec.mode, Some("0700".to_string()));
        assert_eq!(spec.owner, Some("app".to_string()));
        assert_eq!(spec.group, Some("app".to_string()));
    }
}

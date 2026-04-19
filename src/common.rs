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

#[derive(Debug, Deserialize)]
pub struct Task {
    pub name: String,
    pub shell: Option<String>,
    pub command: Option<String>,
    pub register: Option<String>,
    pub debug: Option<Debug>,
    pub vars: Option<IndexMap<String, String>>,
    pub chdir: Option<String>,
    pub when: Option<String>,
    pub r#loop: Option<Vec<Value>>,
    pub include_tasks: Option<String>,
    pub login_shell: Option<bool>,
    pub r#become: Option<bool>,
    pub become_method: Option<String>,
    pub template: Option<TemplateSpec>,
    pub copy: Option<CopySpec>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct TemplateSpec {
    pub src: String,
    pub dest: String,
}

#[derive(Debug, Deserialize)]
pub struct CopySpec {
    pub src: Option<String>,
    pub content: Option<String>,
    pub dest: String,
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
}

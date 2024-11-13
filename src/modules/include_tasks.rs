use crate::common::Task;
use crate::utils;

pub fn process(include_file: &str) -> Result<Vec<Task>, Box<dyn std::error::Error>> {
    let included_tasks: Vec<Task> = utils::read_yaml(include_file)?;
    Ok(included_tasks)
}

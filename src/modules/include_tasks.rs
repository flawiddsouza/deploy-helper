use crate::common::Task;
use crate::utils;

pub fn process(include_file: &str) -> Vec<Task> {
    utils::read_yaml(include_file)
}

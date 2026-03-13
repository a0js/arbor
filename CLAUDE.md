## Rust File Reading
When reading Rust source files, use these practices to minimize token usage:
- **Stop before test modules**: Rust unit tests in `#[cfg(test)]` sections at the bottom of files are rarely needed unless explicitly relevant to the task. Skip them to save tokens.
- **Use Grep to locate functions**: When searching for a specific function or implementation, use Grep instead of reading the entire file. This is more efficient for large files.
- **Prefer targeted searches**: For specific code patterns, function signatures, or implementations, Grep is more efficient than full file reads.
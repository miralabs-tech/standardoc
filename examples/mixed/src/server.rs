/// HTTP handler that greets the caller.
/// @doc server.greet Greet handler
/// @param name string the caller's name
/// @returns string the greeting message
/// @since 0.1.0
pub fn greet(name: &str) -> String {
    format!("Hello, {name}!")
}

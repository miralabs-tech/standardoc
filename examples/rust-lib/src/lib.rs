//! Minimal Rust library illustrating standardoc annotations.

/// A simple calculator for demo purposes.
/// @doc calculator Calculator
/// @since 1.0.0
pub struct Calculator;

impl Calculator {
    /// Adds two integers together.
    /// @doc calculator.add add
    /// @param a i32 the first operand
    /// @param b i32 the second operand
    /// @returns i32 the sum
    /// @example
    /// let r = Calculator::add(2, 3);
    /// assert_eq!(r, 5);
    pub fn add(a: i32, b: i32) -> i32 {
        a + b
    }

    /// Subtracts b from a.
    /// Auto-inferred block — no explicit `@doc`.
    pub fn sub(a: i32, b: i32) -> i32 {
        a - b
    }
}

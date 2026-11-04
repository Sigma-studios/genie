//! `/calc 2*(3+4)`, and the same in the regular search when what's typed is a
//! calculation: Enter copies the result, Tab puts it in the input to go on.
//!
//! `+ - * / % ^`, parentheses, `2pi` for `2*pi`, `pi`, `e`, `tau`, and the
//! usual functions: `sqrt`, `ln`, `log`, `sin`… in radians.

use crate::plugin::Action;
use crate::plugin::Command;
use crate::plugin::Effect;
use crate::plugin::Icon;
use crate::plugin::Item;
use crate::plugin::Plugin;
use crate::plugin::Query;

const ICON: &str = "accessories-calculator";

pub struct Calc;

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64),
    Name(String),
    /// One of `+ - * / % ^ ( )`, with `×`, `÷` and `**` read as theirs.
    Symbol(char),
}

fn tokenize(text: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || c == '.' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            // An exponent, `1e3` or `2.5e-4`, but `2e` is `2*e`.
            if i < chars.len() && matches!(chars[i], 'e' | 'E') {
                let digits = if matches!(chars.get(i + 1), Some('+' | '-')) { i + 2 } else { i + 1 };
                if chars.get(digits).is_some_and(char::is_ascii_digit) {
                    i = digits;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let number: String = chars[start..i].iter().collect();
            tokens.push(Token::Number(number.parse().map_err(|_| format!("{number} isn't a number"))?));
        } else if c.is_alphabetic() || c == 'π' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == 'π') {
                i += 1;
            }
            tokens.push(Token::Name(chars[start..i].iter().collect::<String>().to_lowercase()));
        } else if c == '*' && chars.get(i + 1) == Some(&'*') {
            tokens.push(Token::Symbol('^'));
            i += 2;
        } else {
            let symbol = match c {
                '×' | '·' => '*',
                '÷' => '/',
                '−' => '-',
                '+' | '-' | '*' | '/' | '%' | '^' | '(' | ')' => c,
                _ => return Err(format!("{c} isn't something to calculate with")),
            };
            tokens.push(Token::Symbol(symbol));
            i += 1;
        }
    }
    Ok(tokens)
}

fn constant(name: &str) -> Option<f64> {
    match name {
        "pi" | "π" => Some(std::f64::consts::PI),
        "tau" => Some(std::f64::consts::TAU),
        "e" => Some(std::f64::consts::E),
        _ => None,
    }
}

fn function(name: &str) -> Option<fn(f64) -> f64> {
    Some(match name {
        "sqrt" => f64::sqrt,
        "cbrt" => f64::cbrt,
        "abs" => f64::abs,
        "ln" => f64::ln,
        "log" => f64::log10,
        "log2" => f64::log2,
        "exp" => f64::exp,
        "sin" => f64::sin,
        "cos" => f64::cos,
        "tan" => f64::tan,
        "asin" => f64::asin,
        "acos" => f64::acos,
        "atan" => f64::atan,
        "floor" => f64::floor,
        "ceil" => f64::ceil,
        "round" => f64::round,
        _ => return None,
    })
}

/// Recursive descent: `expr` is sums, `term` products, `unary` signs, `power`
/// is right-associative and binds tighter than a sign, so `-2^2` is -4.
struct Parser {
    tokens: Vec<Token>,
    at: usize,
    /// Whether there was more than a number: `42` or `pi` alone isn't worth
    /// answering in the regular search.
    computed: bool,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn eat(&mut self, symbol: char) -> bool {
        let found = self.peek() == Some(&Token::Symbol(symbol));
        if found {
            self.at += 1;
        }
        found
    }

    fn expr(&mut self) -> Result<f64, String> {
        let mut value = self.term()?;
        loop {
            if self.eat('+') {
                value += self.term()?;
            } else if self.eat('-') {
                value -= self.term()?;
            } else {
                return Ok(value);
            }
            self.computed = true;
        }
    }

    fn term(&mut self) -> Result<f64, String> {
        let mut value = self.unary()?;
        loop {
            if self.eat('*') {
                value *= self.unary()?;
            } else if self.eat('/') {
                value /= self.unary()?;
            } else if self.eat('%') {
                value %= self.unary()?;
            } else if matches!(self.peek(), Some(Token::Symbol('(') | Token::Name(_))) {
                // `2pi`, `3(1+2)`, `2 sqrt 4`.
                value *= self.power()?;
            } else {
                return Ok(value);
            }
            self.computed = true;
        }
    }

    fn unary(&mut self) -> Result<f64, String> {
        if self.eat('-') {
            return Ok(-self.unary()?);
        }
        if self.eat('+') {
            return self.unary();
        }
        self.power()
    }

    fn power(&mut self) -> Result<f64, String> {
        let base = self.primary()?;
        if self.eat('^') {
            self.computed = true;
            return Ok(base.powf(self.unary()?));
        }
        Ok(base)
    }

    fn primary(&mut self) -> Result<f64, String> {
        let token = self.peek().cloned().ok_or("Keep typing")?;
        self.at += 1;
        match token {
            Token::Number(n) => Ok(n),
            Token::Symbol('(') => {
                let value = self.expr()?;
                // A parenthesis left open closes at the end, as you type.
                if !self.eat(')') && self.peek().is_some() {
                    return Err("A parenthesis isn't closed".into());
                }
                Ok(value)
            }
            Token::Name(name) => {
                if let Some(value) = constant(&name) {
                    return Ok(value);
                }
                let f = function(&name).ok_or_else(|| format!("{name} isn't a function or constant I know"))?;
                self.computed = true;
                Ok(f(self.power()?))
            }
            Token::Symbol(symbol) => Err(format!("{symbol} isn't expected here")),
        }
    }
}

/// The value, and whether it took any calculating.
fn evaluate(text: &str) -> Result<(f64, bool), String> {
    let mut parser = Parser { tokens: tokenize(text)?, at: 0, computed: false };
    let value = parser.expr()?;
    if let Some(token) = parser.peek() {
        return Err(match token {
            Token::Symbol(')') => "A parenthesis closes that wasn't opened".into(),
            _ => "Something's left over".into(),
        });
    }
    if !value.is_finite() {
        return Err(if value.is_nan() { "That isn't a number".into() } else { "That's infinite".into() });
    }
    Ok((value, parser.computed))
}

/// Ten significant digits, so `0.1+0.2` is `0.3`, and no `.0` on whole numbers.
fn format(value: f64) -> String {
    let magnitude = value.abs();
    if magnitude == 0.0 {
        return "0".into();
    }
    if !(1e-6..1e15).contains(&magnitude) {
        let text = format!("{value:.9e}");
        let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
        return format!("{}e{exponent}", trim_zeros(mantissa));
    }
    let decimals = (9 - magnitude.log10().floor() as i32).clamp(0, 15) as usize;
    trim_zeros(&format!("{value:.decimals$}"))
}

fn trim_zeros(number: &str) -> String {
    if number.contains('.') { number.trim_end_matches('0').trim_end_matches('.').to_string() } else { number.to_string() }
}

fn result(text: &str, value: f64, completion: String) -> Item {
    let answer = format(value);
    let line = format!("{text} = {answer}");
    Item::new("calc", &answer, format!("{text} =  ·  Enter copies"), Icon::Theme(ICON.into()))
        .action(Action::new("copy", "Copy the result", Effect::Copy(answer.clone())))
        .action(Action::new("copyline", "Copy the calculation", Effect::Copy(line)))
        .completion(completion.replace("{}", &answer))
}

impl Plugin for Calc {
    fn id(&self) -> &'static str {
        "calc"
    }

    fn group(&self) -> &'static str {
        "Tools"
    }

    fn commands(&self) -> Vec<Command> {
        vec![Command::new("calc", &["="], "Calculate, and copy the result. Works in the regular search too", ICON)]
    }

    fn regular(&self) -> bool {
        true
    }

    fn query(&self, query: &Query) {
        let text = query.text.trim();
        if query.command.is_none() {
            // Only what's clearly a calculation: `2+2`, but not `42` or `e`.
            if let Ok((value, true)) = evaluate(text) {
                query.send(vec![result(text, value, "{}".into()).score(2.0)]);
            }
            return;
        }
        let item = if text.is_empty() {
            Item::new("calc.hint", "Calculate", "Type a calculation, like 2*(3+4), 15% 4, sqrt 2 or 2pi", Icon::Theme(ICON.into())).completion("/calc ")
        } else {
            match evaluate(text) {
                Ok((value, _)) => result(text, value, "/calc {}".into()),
                Err(error) => Item::new("calc.error", "…", error, Icon::Theme(ICON.into())),
            }
        };
        query.send(vec![item.score(1.0)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calc(text: &str) -> String {
        evaluate(text).map(|(value, _)| format(value)).unwrap_or_else(|e| format!("error: {e}"))
    }

    #[test]
    fn precedence_and_signs() {
        assert_eq!(calc("2+3*4"), "14");
        assert_eq!(calc("(2+3)*4"), "20");
        assert_eq!(calc("2^3^2"), "512");
        assert_eq!(calc("-2^2"), "-4");
        assert_eq!(calc("2*-3"), "-6");
        assert_eq!(calc("10 - 4 - 3"), "3");
        assert_eq!(calc("7 % 3"), "1");
        assert_eq!(calc("2**10"), "1024");
        assert_eq!(calc("6 × 7 ÷ 2"), "21");
    }

    #[test]
    fn numbers_print_cleanly() {
        assert_eq!(calc("0.1+0.2"), "0.3");
        assert_eq!(calc("1/3"), "0.3333333333");
        assert_eq!(calc("2/3*1000"), "666.6666667");
        assert_eq!(calc("1e3"), "1000");
        assert_eq!(calc("2.5e-4*2"), "0.0005");
        assert_eq!(calc("10^20"), "1e20");
        assert_eq!(calc("1/10^8"), "1e-8");
        assert_eq!(calc("-0"), "0");
    }

    #[test]
    fn functions_constants_and_implicit_products() {
        assert_eq!(calc("sqrt(16)"), "4");
        assert_eq!(calc("sqrt 16 + 1"), "5");
        assert_eq!(calc("2pi"), calc("2*pi"));
        assert_eq!(calc("2e"), calc("2*e"));
        assert_eq!(calc("3(1+2)"), "9");
        assert_eq!(calc("log(1000)"), "3");
        assert_eq!(calc("round(2.5)"), "3");
        assert_eq!(calc("cos(π)"), "-1");
    }

    #[test]
    fn an_open_parenthesis_closes_at_the_end() {
        assert_eq!(calc("2*(3+4"), "14");
        assert!(calc("2*(3+4))").starts_with("error"));
    }

    #[test]
    fn what_isnt_a_calculation_is_refused() {
        for text in ["", "2+", "firefox", "a-b", "1/0", "sqrt(-1)", "2 $ 3", "1..2"] {
            assert!(calc(text).starts_with("error"), "{text}");
        }
    }

    #[test]
    fn the_regular_search_only_answers_calculations() {
        let computed = |text: &str| evaluate(text).is_ok_and(|(_, computed)| computed);
        assert!(computed("2+2"));
        assert!(computed("sqrt 2"));
        assert!(computed("2pi"));
        assert!(!computed("42"));
        assert!(!computed("e"));
        assert!(!computed("pi"));
        assert!(!computed("-5"));
    }
}

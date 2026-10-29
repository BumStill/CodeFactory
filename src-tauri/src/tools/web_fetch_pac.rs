// SPDX-License-Identifier: Apache-2.0
//! M195 — the deliberately small PAC (`FindProxyForURL`) evaluator.
//!
//! A PAC script is arbitrary JavaScript. Pulling in a JS engine for a single
//! read-only routing decision is a lot of binary and attack surface, so this
//! module understands exactly the constructs listed in the M195 spec and
//! returns an error for anything else. `web_fetch_net::route_for` turns that
//! error into a documented fallback instead of guessing a route.

use std::collections::HashMap;

/// Evaluate a PAC script's `FindProxyForURL(url, host)`.
pub(crate) fn evaluate_pac(script: &str, url: &str, host: &str) -> Result<String, String> {
    let tokens = tokenize(script)?;
    PacParser::new(tokens, url, host).run()
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Str(String),
    Num(f64),
    Punct(&'static str),
    Eof,
}

fn tokenize(script: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = script.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
            continue;
        }
        if c == '"' || c == '\'' {
            let quote = c;
            i += 1;
            let mut value = String::new();
            while i < chars.len() && chars[i] != quote {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                    value.push(match chars[i] {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                } else {
                    value.push(chars[i]);
                }
                i += 1;
            }
            if i >= chars.len() {
                return Err("unterminated string literal".to_string());
            }
            i += 1;
            out.push(Token::Str(value));
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            let number = text
                .parse::<f64>()
                .map_err(|_| format!("`{text}` is not a number"))?;
            out.push(Token::Num(number));
            continue;
        }
        if c.is_alphabetic() || c == '_' || c == '$' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$')
            {
                i += 1;
            }
            out.push(Token::Ident(chars[start..i].iter().collect()));
            continue;
        }
        let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
        let two_punct = match two.as_str() {
            "||" => Some("||"),
            "&&" => Some("&&"),
            "==" => Some("=="),
            "!=" => Some("!="),
            "<=" => Some("<="),
            ">=" => Some(">="),
            _ => None,
        };
        if let Some(punct) = two_punct {
            out.push(Token::Punct(punct));
            i += 2;
            continue;
        }
        let one = match c {
            '(' => "(",
            ')' => ")",
            '{' => "{",
            '}' => "}",
            ';' => ";",
            ',' => ",",
            '.' => ".",
            '!' => "!",
            '<' => "<",
            '>' => ">",
            '=' => "=",
            '+' => "+",
            '[' => "[",
            ']' => "]",
            ':' => ":",
            _ => return Err(format!("unexpected character `{c}` in the PAC script")),
        };
        out.push(Token::Punct(one));
        i += 1;
    }
    out.push(Token::Eof);
    Ok(out)
}

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Str(String),
    Num(f64),
    Bool(bool),
    Undefined,
}

impl Value {
    fn truthy(&self) -> bool {
        match self {
            Value::Str(text) => !text.is_empty(),
            Value::Num(number) => *number != 0.0,
            Value::Bool(flag) => *flag,
            Value::Undefined => false,
        }
    }

    fn text(&self) -> String {
        match self {
            Value::Str(text) => text.clone(),
            Value::Num(number) => {
                if number.fract() == 0.0 {
                    format!("{number:.0}")
                } else {
                    number.to_string()
                }
            }
            Value::Bool(flag) => flag.to_string(),
            Value::Undefined => String::new(),
        }
    }

    fn number(&self) -> Option<f64> {
        match self {
            Value::Num(number) => Some(*number),
            Value::Str(text) => text.parse().ok(),
            Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
            Value::Undefined => None,
        }
    }
}

enum Flow {
    Return(Value),
    Next,
}

struct PacParser {
    tokens: Vec<Token>,
    pos: usize,
    url: String,
    host: String,
    locals: HashMap<String, Value>,
}

impl PacParser {
    fn new(tokens: Vec<Token>, url: &str, host: &str) -> Self {
        let mut locals = HashMap::new();
        locals.insert("url".to_string(), Value::Str(url.to_string()));
        locals.insert("host".to_string(), Value::Str(host.to_string()));
        Self {
            tokens,
            pos: 0,
            url: url.to_string(),
            host: host.to_string(),
            locals,
        }
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn next(&mut self) -> Token {
        let token = self.peek().clone();
        self.pos += 1;
        token
    }

    fn eat_punct(&mut self, punct: &str) -> bool {
        if let Token::Punct(value) = self.peek() {
            if *value == punct {
                self.pos += 1;
                return true;
            }
        }
        false
    }

    fn expect_punct(&mut self, punct: &str) -> Result<(), String> {
        if self.eat_punct(punct) {
            Ok(())
        } else {
            Err(format!("expected `{punct}` in the PAC script"))
        }
    }

    fn expect_ident(&mut self) -> Result<String, String> {
        match self.next() {
            Token::Ident(name) => Ok(name),
            other => Err(format!("expected a name in the PAC script, got {other:?}")),
        }
    }

    fn run(&mut self) -> Result<String, String> {
        loop {
            match self.peek().clone() {
                Token::Eof => {
                    return Err("no FindProxyForURL function in the PAC script".to_string())
                }
                Token::Ident(name) if name == "function" => {
                    self.pos += 1;
                    let fname = self.expect_ident()?;
                    if fname != "FindProxyForURL" {
                        self.skip_function()?;
                        continue;
                    }
                    self.expect_punct("(")?;
                    let mut params = Vec::new();
                    while let Token::Ident(param) = self.peek().clone() {
                        params.push(param);
                        self.pos += 1;
                        if !self.eat_punct(",") {
                            break;
                        }
                    }
                    self.expect_punct(")")?;
                    if let Some(name) = params.first() {
                        self.locals.insert(name.clone(), Value::Str(self.url.clone()));
                    }
                    if let Some(name) = params.get(1) {
                        self.locals.insert(name.clone(), Value::Str(self.host.clone()));
                    }
                    self.expect_punct("{")?;
                    return match self.exec_block()? {
                        Flow::Return(value) => Ok(value.text()),
                        Flow::Next => Ok("DIRECT".to_string()),
                    };
                }
                _ => self.pos += 1,
            }
        }
    }

    fn skip_function(&mut self) -> Result<(), String> {
        while !matches!(self.peek(), Token::Punct("{")) {
            if matches!(self.peek(), Token::Eof) {
                return Err("unterminated function in the PAC script".to_string());
            }
            self.pos += 1;
        }
        let mut depth = 0;
        loop {
            match self.next() {
                Token::Punct("{") => depth += 1,
                Token::Punct("}") => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                Token::Eof => {
                    return Err("unterminated function body in the PAC script".to_string())
                }
                _ => {}
            }
        }
    }

    fn exec_block(&mut self) -> Result<Flow, String> {
        loop {
            if self.eat_punct("}") {
                return Ok(Flow::Next);
            }
            if matches!(self.peek(), Token::Eof) {
                return Err("unterminated block in the PAC script".to_string());
            }
            match self.exec_statement()? {
                Flow::Return(value) => return Ok(Flow::Return(value)),
                Flow::Next => {}
            }
        }
    }

    fn exec_statement(&mut self) -> Result<Flow, String> {
        if self.eat_punct(";") {
            return Ok(Flow::Next);
        }
        if self.eat_punct("{") {
            return self.exec_block();
        }
        if let Token::Ident(name) = self.peek().clone() {
            match name.as_str() {
                "if" => {
                    self.pos += 1;
                    self.expect_punct("(")?;
                    let condition = self.expr()?;
                    self.expect_punct(")")?;
                    if condition.truthy() {
                        return self.exec_statement();
                    }
                    self.skip_statement()?;
                    if matches!(self.peek(), Token::Ident(word) if word == "else") {
                        self.pos += 1;
                        return self.exec_statement();
                    }
                    return Ok(Flow::Next);
                }
                "return" => {
                    self.pos += 1;
                    let value = self.expr()?;
                    self.eat_punct(";");
                    return Ok(Flow::Return(value));
                }
                "var" | "let" | "const" => {
                    self.pos += 1;
                    loop {
                        let name = self.expect_ident()?;
                        let value = if self.eat_punct("=") {
                            self.expr()?
                        } else {
                            Value::Undefined
                        };
                        self.locals.insert(name, value);
                        self.eat_punct(";");
                        if !self.eat_punct(",") {
                            break;
                        }
                    }
                    return Ok(Flow::Next);
                }
                _ => {
                    let start = self.pos;
                    self.pos += 1;
                    if self.eat_punct("=") {
                        let value = self.expr()?;
                        self.eat_punct(";");
                        self.locals.insert(name, value);
                        return Ok(Flow::Next);
                    }
                    self.pos = start;
                }
            }
        }
        Err(format!(
            "unsupported PAC statement starting at token {:?}",
            self.peek()
        ))
    }

    fn skip_statement(&mut self) -> Result<(), String> {
        if self.eat_punct(";") {
            return Ok(());
        }
        if self.eat_punct("{") {
            let mut depth = 1;
            loop {
                match self.next() {
                    Token::Punct("{") => depth += 1,
                    Token::Punct("}") => {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(());
                        }
                    }
                    Token::Eof => {
                        return Err("unterminated block in the PAC script".to_string())
                    }
                    _ => {}
                }
            }
        }
        loop {
            match self.peek() {
                Token::Punct(";") => {
                    self.pos += 1;
                    return Ok(());
                }
                Token::Punct("}") | Token::Eof => return Ok(()),
                _ => self.pos += 1,
            }
        }
    }

    fn expr(&mut self) -> Result<Value, String> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> Result<Value, String> {
        let mut left = self.and_expr()?;
        while self.eat_punct("||") {
            let right = self.and_expr()?;
            left = Value::Bool(left.truthy() || right.truthy());
        }
        Ok(left)
    }

    fn and_expr(&mut self) -> Result<Value, String> {
        let mut left = self.compare_expr()?;
        while self.eat_punct("&&") {
            let right = self.compare_expr()?;
            left = Value::Bool(left.truthy() && right.truthy());
        }
        Ok(left)
    }

    fn compare_expr(&mut self) -> Result<Value, String> {
        let left = self.add_expr()?;
        for operator in ["==", "!=", "<=", ">=", "<", ">"] {
            if self.eat_punct(operator) {
                let right = self.add_expr()?;
                let flag = match operator {
                    "==" => left.text() == right.text(),
                    "!=" => left.text() != right.text(),
                    _ => match (left.number(), right.number()) {
                        (Some(a), Some(b)) => match operator {
                            "<=" => a <= b,
                            ">=" => a >= b,
                            "<" => a < b,
                            _ => a > b,
                        },
                        _ => match operator {
                            "<=" => left.text() <= right.text(),
                            ">=" => left.text() >= right.text(),
                            "<" => left.text() < right.text(),
                            _ => left.text() > right.text(),
                        },
                    },
                };
                return Ok(Value::Bool(flag));
            }
        }
        Ok(left)
    }

    fn add_expr(&mut self) -> Result<Value, String> {
        let mut left = self.unary()?;
        while self.eat_punct("+") {
            let right = self.unary()?;
            left = Value::Str(format!("{}{}", left.text(), right.text()));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Value, String> {
        if self.eat_punct("!") {
            let value = self.unary()?;
            return Ok(Value::Bool(!value.truthy()));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Value, String> {
        match self.next() {
            Token::Str(text) => Ok(Value::Str(text)),
            Token::Num(number) => Ok(Value::Num(number)),
            Token::Punct("(") => {
                let value = self.expr()?;
                self.expect_punct(")")?;
                Ok(value)
            }
            Token::Ident(name) => {
                if self.eat_punct("(") {
                    let args = self.arguments()?;
                    return self.call(&name, args);
                }
                if self.eat_punct(".") {
                    let method = self.expect_ident()?;
                    self.expect_punct("(")?;
                    let args = self.arguments()?;
                    let receiver = self
                        .locals
                        .get(&name)
                        .cloned()
                        .unwrap_or(Value::Undefined);
                    return string_method(&receiver.text(), &method, args);
                }
                match name.as_str() {
                    "true" => Ok(Value::Bool(true)),
                    "false" => Ok(Value::Bool(false)),
                    "null" | "undefined" => Ok(Value::Undefined),
                    _ => Ok(self.locals.get(&name).cloned().unwrap_or(Value::Undefined)),
                }
            }
            other => Err(format!("unexpected token {other:?} in the PAC script")),
        }
    }

    /// Consume `arg, arg)` — the opening paren is already eaten.
    fn arguments(&mut self) -> Result<Vec<Value>, String> {
        let mut args = Vec::new();
        if self.eat_punct(")") {
            return Ok(args);
        }
        loop {
            args.push(self.expr()?);
            if self.eat_punct(")") {
                return Ok(args);
            }
            self.expect_punct(",")?;
        }
    }

    fn call(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        let text = |index: usize| {
            args.get(index)
                .map(|value| value.text())
                .unwrap_or_default()
        };
        match name {
            "shExpMatch" => Ok(Value::Bool(glob_match(&text(0), &text(1)))),
            "dnsDomainIs" => {
                let host = text(0).to_ascii_lowercase();
                let domain = text(1).to_ascii_lowercase();
                Ok(Value::Bool(!domain.is_empty() && host.ends_with(&domain)))
            }
            "isPlainHostName" => Ok(Value::Bool(!text(0).contains('.'))),
            "localHostOrDomainIs" => {
                let host = text(0).to_ascii_lowercase();
                let domain = text(1).to_ascii_lowercase();
                Ok(Value::Bool(
                    host == domain || domain.ends_with(&format!(".{host}")),
                ))
            }
            "isInNet" => Ok(Value::Bool(is_in_net(&text(0), &text(1), &text(2)))),
            "dnsResolve" => Ok(Value::Str(resolve_first_ipv4(&text(0)).unwrap_or_default())),
            "isResolvable" => Ok(Value::Bool(resolve_first_ipv4(&text(0)).is_some())),
            other => Err(format!(
                "the PAC function `{other}` is not supported, so web_fetch will not guess a route"
            )),
        }
    }
}

fn string_method(receiver: &str, method: &str, args: Vec<Value>) -> Result<Value, String> {
    let text = |index: usize| {
        args.get(index)
            .map(|value| value.text())
            .unwrap_or_default()
    };
    let number = |index: usize| args.get(index).and_then(|value| value.number());
    match method {
        "toLowerCase" => Ok(Value::Str(receiver.to_ascii_lowercase())),
        "toUpperCase" => Ok(Value::Str(receiver.to_ascii_uppercase())),
        "indexOf" => Ok(Value::Num(
            receiver
                .find(&text(0))
                .map(|index| index as f64)
                .unwrap_or(-1.0),
        )),
        "substring" | "substr" => {
            let chars: Vec<char> = receiver.chars().collect();
            let start = number(0).unwrap_or(0.0).max(0.0) as usize;
            let end = number(1)
                .map(|value| value.max(0.0) as usize)
                .unwrap_or(chars.len());
            let (start, end) = (start.min(chars.len()), end.min(chars.len()));
            let (start, end) = if start <= end {
                (start, end)
            } else {
                (end, start)
            };
            Ok(Value::Str(chars[start..end].iter().collect()))
        }
        other => Err(format!(
            "the PAC string method `{other}` is not supported, so web_fetch will not guess a route"
        )),
    }
}

/// `shExpMatch` glob: `*` and `?` only.
fn glob_match(value: &str, pattern: &str) -> bool {
    fn helper(value: &[char], pattern: &[char]) -> bool {
        match pattern.split_first() {
            None => value.is_empty(),
            Some(('*', rest)) => (0..=value.len()).any(|skip| helper(&value[skip..], rest)),
            Some(('?', rest)) => !value.is_empty() && helper(&value[1..], rest),
            Some((c, rest)) => value.first() == Some(c) && helper(&value[1..], rest),
        }
    }
    let value: Vec<char> = value.chars().collect();
    let pattern: Vec<char> = pattern.chars().collect();
    helper(&value, &pattern)
}

/// `isInNet(host, pattern, mask)` — literal addresses first, then the system
/// resolver (the same one the browser uses; no alternate DNS).
fn is_in_net(host: &str, pattern: &str, mask: &str) -> bool {
    use std::net::Ipv4Addr;
    let (Ok(pattern), Ok(mask)) = (pattern.parse::<Ipv4Addr>(), mask.parse::<Ipv4Addr>()) else {
        return false;
    };
    let Some(address) = host
        .parse::<Ipv4Addr>()
        .ok()
        .or_else(|| resolve_first_ipv4(host).and_then(|text| text.parse().ok()))
    else {
        return false;
    };
    let (address, pattern, mask) = (u32::from(address), u32::from(pattern), u32::from(mask));
    address & mask == pattern & mask
}

fn resolve_first_ipv4(host: &str) -> Option<String> {
    use std::net::ToSocketAddrs;
    if host.is_empty() {
        return None;
    }
    (host, 0u16)
        .to_socket_addrs()
        .ok()?
        .into_iter()
        .find_map(|address| match address.ip() {
            std::net::IpAddr::V4(v4) => Some(v4.to_string()),
            std::net::IpAddr::V6(_) => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pac(body: &str) -> String {
        format!("function FindProxyForURL(url, host) {{{body}}}")
    }

    #[test]
    fn m195_pac_glob_covers_star_and_question_mark() {
        let script = pac(
            r#"
            if (shExpMatch(host, "*.corp.example")) { return "DIRECT"; }
            if (shExpMatch(host, "web??.example")) { return "PROXY web:8080"; }
            return "PROXY proxy:3128";
            "#,
        );
        assert_eq!(
            evaluate_pac(&script, "https://wiki.corp.example/a", "wiki.corp.example").unwrap(),
            "DIRECT"
        );
        assert_eq!(
            evaluate_pac(&script, "http://web01.example/", "web01.example").unwrap(),
            "PROXY web:8080"
        );
        assert_eq!(
            evaluate_pac(&script, "https://metr.org/", "metr.org").unwrap(),
            "PROXY proxy:3128"
        );
    }

    #[test]
    fn m195_pac_domain_and_plain_host_helpers() {
        let script = pac(
            r#"
            if (isPlainHostName(host)) { return "DIRECT"; }
            if (dnsDomainIs(host, ".internal.example")) { return "DIRECT"; }
            if (localHostOrDomainIs(host, "wiki.example")) { return "PROXY local:1"; }
            return "PROXY proxy:3128";
            "#,
        );
        assert_eq!(evaluate_pac(&script, "http://box/", "box").unwrap(), "DIRECT");
        assert_eq!(
            evaluate_pac(&script, "http://a.internal.example/", "a.internal.example").unwrap(),
            "DIRECT"
        );
        assert_eq!(
            evaluate_pac(&script, "http://wiki.example/", "wiki.example").unwrap(),
            "PROXY local:1"
        );
        assert_eq!(
            evaluate_pac(&script, "http://metr.org/", "metr.org").unwrap(),
            "PROXY proxy:3128"
        );
    }

    #[test]
    fn m195_pac_is_in_net_uses_literal_addresses() {
        // Literal addresses only: resolving a public name here would depend on
        // the machine's DNS (fake-IP setups answer 198.18.0.0/16 for everything).
        let script = pac(
            r#"
            if (isInNet(host, "198.18.0.0", "255.254.0.0")) { return "PROXY fakeip:1"; }
            if (isInNet(host, "10.0.0.0", "255.0.0.0")) { return "DIRECT"; }
            return "PROXY proxy:3128";
            "#,
        );
        assert_eq!(
            evaluate_pac(&script, "http://198.18.3.4/", "198.18.3.4").unwrap(),
            "PROXY fakeip:1"
        );
        assert_eq!(
            evaluate_pac(&script, "http://10.2.3.4/", "10.2.3.4").unwrap(),
            "DIRECT"
        );
        assert_eq!(
            evaluate_pac(&script, "http://203.0.113.9/", "203.0.113.9").unwrap(),
            "PROXY proxy:3128"
        );
    }

    #[test]
    fn m195_pac_string_methods_and_else_branches() {
        let script = pac(
            r#"
            var lowered = host.toLowerCase();
            if (lowered.substring(0, 4) == "wiki") { return "DIRECT"; }
            else if (lowered.indexOf("corp") >= 0) { return "PROXY corp:8080"; }
            return "PROXY proxy:3128";
            "#,
        );
        assert_eq!(
            evaluate_pac(&script, "http://wiki.example/", "WIKI.example").unwrap(),
            "DIRECT"
        );
        assert_eq!(
            evaluate_pac(&script, "http://a.corp.example/", "a.corp.example").unwrap(),
            "PROXY corp:8080"
        );
    }

    #[test]
    fn m195_pac_reports_unsupported_functions_instead_of_guessing() {
        let script = pac(r#" if (weekdayRange("SAT", "SUN")) { return "DIRECT"; } return "DIRECT"; "#);
        let error = evaluate_pac(&script, "https://metr.org/", "metr.org")
            .expect_err("unsupported PAC function must be an error");
        assert!(error.contains("weekdayRange"), "unexpected error: {error}");
    }

    #[test]
    fn m195_pac_without_find_proxy_for_url_is_an_error() {
        let error = evaluate_pac("// nothing here", "https://metr.org/", "metr.org")
            .expect_err("a PAC without FindProxyForURL must be an error");
        assert!(error.contains("FindProxyForURL"), "unexpected error: {error}");
    }
}

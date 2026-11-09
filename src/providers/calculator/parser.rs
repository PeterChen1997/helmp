//! 算式的规范化、词法、「像不像算式」的判断与递归下降求值。
//!
//! 文法（优先级由低到高）：
//! ```text
//! expr    := term (('+' | '-') term)*
//! term    := unary (('*' | '/' | '%') unary)*
//! unary   := ('+' | '-') unary | power
//! power   := primary (('^' | '**') unary)?      右结合：2^3^2 = 2^9，-2^2 = -4
//! primary := number | const | name '(' args ')' | '(' expr ')'
//! ```

/// 计算失败的原因。界面按种类给出具体说明，而不是笼统的「错误」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalcError {
    DivisionByZero,
    /// 写到一半：缺操作数或缺右括号。
    Incomplete,
    /// 结果或中间值超出 f64 范围。
    OutOfRange,
    /// 超出函数定义域（负数开平方、对负数取对数）。
    Domain,
    UnknownFunction(String),
    WrongArity {
        name: String,
        expected: &'static str,
    },
    /// 其他写法错误（多余的括号、连着两个运算符）。
    Syntax,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Num(f64),
    Ident(String),
    Op(char),
    LParen,
    RParen,
    Comma,
}

/// 括号嵌套上限，防止恶意长输入把栈打爆。
const MAX_DEPTH: usize = 64;

/// 全角数字与符号、中文括号、`×÷`、减号变体 → ASCII。
pub fn normalize(input: &str) -> String {
    input
        .chars()
        .map(|c| match c {
            // 全角 ASCII 区：（）＋－＊／％＾．，０-９ａ-ｚ 等，与半角相差 0xFEE0。
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{3000}' => ' ',
            '×' | '✕' | '✖' | '⋅' | '·' => '*',
            '÷' => '/',
            '−' | '–' | '—' => '-',
            '。' => '.',
            other => other,
        })
        .collect()
}

fn tokenize(text: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        match c {
            _ if c.is_whitespace() => i += 1,
            '0'..='9' | '.' => {
                let start = i;
                while chars.get(i).is_some_and(char::is_ascii_digit) {
                    i += 1;
                }
                if chars.get(i) == Some(&'.') {
                    i += 1;
                    while chars.get(i).is_some_and(char::is_ascii_digit) {
                        i += 1;
                    }
                }
                // 科学计数：e 后必须跟数字（可带符号），否则 `2e` 是 2 和常数 e。
                if matches!(chars.get(i), Some('e' | 'E')) {
                    let mut j = i + 1;
                    if matches!(chars.get(j), Some('+' | '-')) {
                        j += 1;
                    }
                    if chars.get(j).is_some_and(char::is_ascii_digit) {
                        while chars.get(j).is_some_and(char::is_ascii_digit) {
                            j += 1;
                        }
                        i = j;
                    }
                }
                let literal: String = chars.get(start..i)?.iter().collect();
                tokens.push(Token::Num(literal.parse().ok()?));
            }
            'a'..='z' | 'A'..='Z' | '_' | 'π' => {
                let start = i;
                if c == 'π' {
                    i += 1;
                } else {
                    while chars
                        .get(i)
                        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
                    {
                        i += 1;
                    }
                }
                let name: String = chars.get(start..i)?.iter().collect();
                tokens.push(Token::Ident(name.to_lowercase()));
            }
            '*' if chars.get(i + 1) == Some(&'*') => {
                tokens.push(Token::Op('^'));
                i += 2;
            }
            '+' | '-' | '*' | '/' | '%' | '^' => {
                tokens.push(Token::Op(c));
                i += 1;
            }
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            _ => return None,
        }
    }
    Some(tokens)
}

fn constant(name: &str) -> Option<f64> {
    match name {
        "pi" | "π" => Some(std::f64::consts::PI),
        "e" => Some(std::f64::consts::E),
        _ => None,
    }
}

const FUNCTIONS: &[&str] = &[
    "sqrt", "cbrt", "abs", "sin", "cos", "tan", "asin", "acos", "atan", "ln", "log", "log2",
    "log10", "exp", "floor", "ceil", "round", "pow", "min", "max",
];

/// 规范化后的文本是不是一道算式。不是就返回 `None`（不出计算结果，也不出错误行）；
/// 是就返回求值结果（可能是错误）。
///
/// 判定：只含数字、运算符、括号与已知名字，且至少有一个二元运算符或函数调用。
/// 于是 `wx`、`微信`、`1password`、`2026`、`-5`、`1.2.3` 都不算；`1+`、`1/0` 算（给出错误说明）。
/// 不认识的名字只有后面紧跟括号时才当成函数（`foo(2)` → 未知函数），否则整句不算算式。
pub fn evaluate(text: &str) -> Option<Result<f64, CalcError>> {
    let tokens = tokenize(text)?;
    let mut signal = false;
    for (index, token) in tokens.iter().enumerate() {
        let next = tokens.get(index + 1);
        match token {
            Token::Ident(name) => {
                let call = next == Some(&Token::LParen);
                if call {
                    signal = true;
                } else if constant(name).is_none() {
                    return None;
                }
            }
            Token::Op(_) => {
                let binary = index
                    .checked_sub(1)
                    .and_then(|prev| tokens.get(prev))
                    .is_some_and(|prev| {
                        matches!(prev, Token::Num(_) | Token::Ident(_) | Token::RParen)
                    });
                signal |= binary;
            }
            _ => {}
        }
    }
    if !signal {
        return None;
    }
    let mut parser = Parser {
        tokens,
        pos: 0,
        depth: 0,
    };
    Some(parser.parse())
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
}

type Calc = Result<f64, CalcError>;

/// 每一步都检查：NaN 是出了定义域，无穷是超出范围。
fn checked(value: f64) -> Calc {
    if value.is_nan() {
        Err(CalcError::Domain)
    } else if value.is_infinite() {
        Err(CalcError::OutOfRange)
    } else {
        Ok(value)
    }
}

impl Parser {
    fn parse(&mut self) -> Calc {
        let value = self.expr()?;
        match self.peek() {
            None => Ok(value),
            Some(_) => Err(CalcError::Syntax),
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        token
    }

    fn eat_op(&mut self, ops: &[char]) -> Option<char> {
        match self.peek() {
            Some(Token::Op(op)) if ops.contains(op) => {
                let op = *op;
                self.pos += 1;
                Some(op)
            }
            _ => None,
        }
    }

    fn expr(&mut self) -> Calc {
        let mut value = self.term()?;
        while let Some(op) = self.eat_op(&['+', '-']) {
            let rhs = self.term()?;
            value = checked(if op == '+' { value + rhs } else { value - rhs })?;
        }
        Ok(value)
    }

    fn term(&mut self) -> Calc {
        let mut value = self.unary()?;
        while let Some(op) = self.eat_op(&['*', '/', '%']) {
            let rhs = self.unary()?;
            value = match op {
                '*' => checked(value * rhs)?,
                _ if rhs == 0.0 => return Err(CalcError::DivisionByZero),
                '/' => checked(value / rhs)?,
                _ => checked(value % rhs)?,
            };
        }
        Ok(value)
    }

    fn unary(&mut self) -> Calc {
        match self.eat_op(&['+', '-']) {
            Some('-') => Ok(-self.nested(Self::unary)?),
            Some(_) => self.nested(Self::unary),
            None => self.power(),
        }
    }

    fn power(&mut self) -> Calc {
        let base = self.primary()?;
        if self.eat_op(&['^']).is_none() {
            return Ok(base);
        }
        let exponent = self.nested(Self::unary)?;
        if base == 0.0 && exponent < 0.0 {
            return Err(CalcError::DivisionByZero);
        }
        checked(base.powf(exponent))
    }

    /// 递归进入下一层，超过嵌套上限按写法错误处理。
    fn nested(&mut self, f: fn(&mut Self) -> Calc) -> Calc {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(CalcError::Syntax);
        }
        let value = f(self);
        self.depth -= 1;
        value
    }

    fn primary(&mut self) -> Calc {
        match self.next() {
            None => Err(CalcError::Incomplete),
            Some(Token::Num(value)) => checked(value),
            Some(Token::LParen) => {
                let value = self.nested(Self::expr)?;
                match self.next() {
                    Some(Token::RParen) => Ok(value),
                    None => Err(CalcError::Incomplete),
                    Some(_) => Err(CalcError::Syntax),
                }
            }
            Some(Token::Ident(name)) => {
                if self.peek() != Some(&Token::LParen) {
                    return constant(&name).ok_or(CalcError::UnknownFunction(name));
                }
                self.pos += 1;
                let args = self.args()?;
                call(&name, &args)
            }
            Some(Token::Op(_) | Token::RParen | Token::Comma) => Err(CalcError::Syntax),
        }
    }

    /// 已吃掉左括号：逗号分隔的参数，直到右括号。
    fn args(&mut self) -> Result<Vec<f64>, CalcError> {
        let mut args = Vec::new();
        if self.peek() == Some(&Token::RParen) {
            self.pos += 1;
            return Ok(args);
        }
        loop {
            args.push(self.nested(Self::expr)?);
            match self.next() {
                Some(Token::Comma) => {}
                Some(Token::RParen) => return Ok(args),
                None => return Err(CalcError::Incomplete),
                Some(_) => return Err(CalcError::Syntax),
            }
        }
    }
}

fn call(name: &str, args: &[f64]) -> Calc {
    if !FUNCTIONS.contains(&name) {
        return Err(CalcError::UnknownFunction(name.to_string()));
    }
    let arity = |expected: &'static str| CalcError::WrongArity {
        name: name.to_string(),
        expected,
    };
    match name {
        "pow" => match args {
            [base, exponent] => {
                if *base == 0.0 && *exponent < 0.0 {
                    return Err(CalcError::DivisionByZero);
                }
                checked(base.powf(*exponent))
            }
            _ => Err(arity("2")),
        },
        "min" | "max" => {
            let mut values = args.iter().copied();
            let first = values.next().ok_or_else(|| arity("1+"))?;
            Ok(values.fold(first, |acc, v| {
                if name == "min" {
                    acc.min(v)
                } else {
                    acc.max(v)
                }
            }))
        }
        _ => {
            let [x] = args else {
                return Err(arity("1"));
            };
            let x = *x;
            let value = match name {
                "sqrt" => x.sqrt(),
                "cbrt" => x.cbrt(),
                "abs" => x.abs(),
                "sin" => snap(x.sin()),
                "cos" => snap(x.cos()),
                "tan" => snap(x.tan()),
                "asin" => x.asin(),
                "acos" => x.acos(),
                "atan" => x.atan(),
                "ln" => log(x, f64::ln)?,
                "log" | "log10" => log(x, f64::log10)?,
                "log2" => log(x, f64::log2)?,
                "exp" => x.exp(),
                "floor" => x.floor(),
                "ceil" => x.ceil(),
                _ => x.round(),
            };
            checked(value)
        }
    }
}

/// 对数：负数出定义域，0 是负无穷（超出范围）。
fn log(x: f64, f: fn(f64) -> f64) -> Calc {
    if x < 0.0 {
        return Err(CalcError::Domain);
    }
    checked(f(x))
}

/// 三角函数在 π 的整数倍上本该是 0，浮点会算出 1e-16 这样的残差：抹成 0，免得显示成科学计数。
fn snap(value: f64) -> f64 {
    if value.abs() < 1e-15 { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calc(text: &str) -> Option<Result<f64, CalcError>> {
        evaluate(&normalize(text))
    }

    fn value(text: &str) -> f64 {
        match calc(text) {
            Some(Ok(value)) => value,
            other => panic!("{text:?} → {other:?}"),
        }
    }

    #[test]
    fn evaluates_with_conventional_precedence() {
        // 优先级错了会给出看起来合理但错误的答案，比报错更糟。
        assert_eq!(value("1+2*3"), 7.0);
        assert_eq!(value("(4-1)^2"), 9.0);
        assert_eq!(value("2**10"), 1024.0);
        assert_eq!(value("10%3"), 1.0);
        assert_eq!(value("2^3^2"), 512.0, "乘方右结合");
        assert_eq!(value("-2^2"), -4.0, "乘方优先于负号");
        assert_eq!(value("2^-1"), 0.5);
        assert_eq!(value("1e3/8"), 125.0);
        assert_eq!(value("max(1, 5, 3) - min(4,2)"), 3.0);
        assert!((value("sqrt(2)") - std::f64::consts::SQRT_2).abs() < 1e-12);
        assert_eq!(value("sin(pi/2)"), 1.0);
        assert_eq!(value("sin(pi)"), 0.0, "π 的残差要抹掉");
        assert_eq!(value("2*π"), std::f64::consts::TAU);
    }

    #[test]
    fn accepts_full_width_and_chinese_symbols() {
        // 中文输入法下打出来的是全角符号与中文括号，不能因此认不出算式。
        assert_eq!(value("（１＋２）×３"), 9.0);
        assert_eq!(value("8÷2"), 4.0);
        assert_eq!(value("１０％３"), 1.0);
        assert_eq!(value("2＾3"), 8.0);
        assert_eq!(value("1。5*2"), 3.0);
    }

    #[test]
    fn plain_search_terms_are_not_expressions() {
        // 这些都是常见搜索词：一旦被当成算式，计算结果或错误行就会顶到应用前面。
        for text in [
            "wx",
            "微信",
            "1password",
            "2026",
            "-5",
            "(5)",
            "1.2.3",
            "c++",
            "e",
            "pi",
            "e-mail",
            "3d",
            "Visual Studio Code",
            "iphone 15",
            "",
            "关机 !",
        ] {
            assert_eq!(calc(text), None, "{text:?}");
        }
    }

    #[test]
    fn errors_say_what_went_wrong() {
        // HapiGo 把错误分得很细；只给一个「错误」用户不知道该改哪里。
        assert_eq!(calc("1/0"), Some(Err(CalcError::DivisionByZero)));
        assert_eq!(calc("5%0"), Some(Err(CalcError::DivisionByZero)));
        assert_eq!(calc("0^-1"), Some(Err(CalcError::DivisionByZero)));
        assert_eq!(calc("1+"), Some(Err(CalcError::Incomplete)));
        assert_eq!(calc("(1+2"), Some(Err(CalcError::Incomplete)));
        assert_eq!(calc("sqrt(2"), Some(Err(CalcError::Incomplete)));
        assert_eq!(calc("10^400"), Some(Err(CalcError::OutOfRange)));
        assert_eq!(calc("1e999+1"), Some(Err(CalcError::OutOfRange)));
        assert_eq!(calc("sqrt(-1)"), Some(Err(CalcError::Domain)));
        assert_eq!(calc("ln(-1)"), Some(Err(CalcError::Domain)));
        assert_eq!(
            calc("foo(2)"),
            Some(Err(CalcError::UnknownFunction("foo".into())))
        );
        assert_eq!(
            calc("sqrt(1,2)"),
            Some(Err(CalcError::WrongArity {
                name: "sqrt".into(),
                expected: "1"
            }))
        );
        assert_eq!(calc("(1+2))"), Some(Err(CalcError::Syntax)));
        assert_eq!(calc("1+*2"), Some(Err(CalcError::Syntax)));
    }

    #[test]
    fn deep_nesting_is_an_error_not_a_crash() {
        // 递归下降在病态输入上会爆栈，启动器每次按键都会算一遍，不能因此崩溃。
        let text = format!("{}1{}+1", "(".repeat(10_000), ")".repeat(10_000));
        assert_eq!(calc(&text), Some(Err(CalcError::Syntax)));
        let unary = format!("{}1+1", "-".repeat(10_000));
        assert_eq!(calc(&unary), Some(Err(CalcError::Syntax)));
    }
}

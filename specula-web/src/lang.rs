//! The page's language: the browser's list matched against the translations
//! bake-translations made, or the one the settings menu names, and the
//! engine's `translate`, `translateWithContext` and `translatePlural` over
//! the loaded table. A string with no entry stays as the English msgid, as
//! with the engine.

use std::cell::RefCell;
use std::collections::HashMap;

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;

/// `localStorage` key of the chosen language code; absent or empty is the
/// browser's list.
pub const SETTING_KEY: &str = "specula.lang";
/// What gettext puts between a context and its msgid.
const CONTEXT_SEPARATOR: char = '\u{4}';

/// One line of `lang/langs.txt`.
#[derive(Clone)]
pub struct Language {
    /// The engine's locale, `pt_BR`.
    pub code: String,
    pub name: String,
    file: String,
}

enum Message {
    One(String),
    Forms(Vec<String>),
}

#[derive(Default)]
struct Table {
    plural: Option<Plural>,
    messages: HashMap<String, Message>,
}

thread_local! {
    static TABLE: RefCell<Table> = RefCell::new(Table::default());
}

/// The engine's `translate`.
pub fn translate(msgid: &str) -> String {
    // The empty msgid is the table's header, not a string.
    if msgid.is_empty() {
        return String::new();
    }
    lookup(msgid)
        .map(|message| first(message, msgid))
        .unwrap_or_else(|| msgid.to_string())
}

/// The engine's `translateWithContext`.
pub fn translate_context(context: &str, msgid: &str) -> String {
    lookup(&format!("{context}{CONTEXT_SEPARATOR}{msgid}"))
        .map(|message| first(message, msgid))
        .unwrap_or_else(|| msgid.to_string())
}

/// The engine's `translatePlural`.
pub fn translate_plural(singular: &str, plural: &str, count: f64) -> String {
    plural_form(singular, plural, count, singular)
}

/// English picks its form by `count != 1`, which is also what a table that
/// has no entry falls back to.
fn plural_form(key: &str, plural: &str, count: f64, singular: &str) -> String {
    let english = || if count == 1.0 { singular } else { plural }.to_string();
    TABLE.with(|table| {
        let table = table.borrow();
        match table.messages.get(key) {
            Some(Message::Forms(forms)) => {
                let at = table
                    .plural
                    .as_ref()
                    .map_or(usize::from(count != 1.0), |rule| rule.form(count));
                forms.get(at).cloned().unwrap_or_else(english)
            }
            Some(Message::One(text)) => text.clone(),
            None => english(),
        }
    })
}

fn lookup(key: &str) -> Option<Message> {
    TABLE.with(|table| {
        table
            .borrow()
            .messages
            .get(key)
            .map(|message| match message {
                Message::One(text) => Message::One(text.clone()),
                Message::Forms(forms) => Message::Forms(forms.clone()),
            })
    })
}

fn first(message: Message, fallback: &str) -> String {
    match message {
        Message::One(text) => text,
        Message::Forms(forms) => forms
            .into_iter()
            .next()
            .unwrap_or_else(|| fallback.to_string()),
    }
}

/// Sets `element`'s text to `english` translated, and keeps the English where
/// i18n.js reads it, so the next language change translates it again.
pub fn set_text(element: &web_sys::HtmlElement, english: &str) {
    let _ = element.set_attribute("data-i18n", "");
    let _ = element.set_attribute("data-i18n-en", english);
    element.set_text_content(Some(&translate(english)));
}

/// Makes `window.speculaTranslate`, for the inline scripts, once.
fn expose() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let name = JsValue::from_str("speculaTranslate");
    if js_sys::Reflect::has(&window, &name).unwrap_or(true) {
        return;
    }
    let closure = Closure::<dyn Fn(String) -> String>::new(|msgid: String| translate(&msgid));
    let _ = js_sys::Reflect::set(&window, &name, closure.as_ref());
    closure.forget();
}

/// Translates the page's marked text again, which i18n.js does.
fn apply_dom() {
    let Some(window) = web_sys::window() else {
        return;
    };
    if let Ok(apply) = js_sys::Reflect::get(&window, &"speculaApplyLanguage".into())
        && let Ok(apply) = apply.dyn_into::<js_sys::Function>()
    {
        let _ = apply.call0(&JsValue::NULL);
    }
}

/// For a page with no language list: the saved language, else the
/// browser's.
pub fn start_page() {
    wasm_bindgen_futures::spawn_local(async {
        let saved = web_sys::window()
            .and_then(|window| window.local_storage().ok().flatten())
            .and_then(|storage| storage.get_item(SETTING_KEY).ok().flatten())
            .unwrap_or_default();
        let languages = languages().await;
        load(choose(&languages, &saved)).await;
    });
}

/// The engine's `sprintf` with named placeholders: each `%(name)s` in
/// `template` becomes the value named `name`.
pub fn format(template: &str, values: &[(&str, &str)]) -> String {
    let mut text = template.to_string();
    for (name, value) in values {
        text = text.replace(&format!("%({name})s"), value);
    }
    text
}

/// The languages the bake made, in menu order; empty if the list cannot be
/// fetched, which leaves the page in English.
pub async fn languages() -> Vec<Language> {
    let Some(text) = fetch_text("/lang/langs.txt").await else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            Some(Language {
                code: parts.next()?.to_string(),
                name: parts.next()?.to_string(),
                file: parts.next()?.to_string(),
            })
        })
        .collect()
}

/// The language `saved` names, else the first of the browser's list that
/// `languages` has, else `None` for English. A tag matches its own code
/// (`pt-BR` is `pt_BR`), then its base language (`de-AT` is `de`); a base
/// language with several codes (`pt`, `zh`) takes the one that is the bare
/// code, else the first.
pub fn choose<'a>(languages: &'a [Language], saved: &str) -> Option<&'a Language> {
    if !saved.is_empty() {
        return languages.iter().find(|language| language.code == saved);
    }
    browser_languages().iter().find_map(|tag| {
        let code = tag.replace('-', "_");
        let base = code.split('_').next().unwrap_or_default();
        if base == "en" {
            // The sources are English; en_GB only differs in spelling.
            return languages
                .iter()
                .find(|language| language.code == code && code != "en");
        }
        languages
            .iter()
            .find(|language| language.code == code)
            .or_else(|| languages.iter().find(|language| language.code == base))
            .or_else(|| {
                languages
                    .iter()
                    .find(|language| language.code.split('_').next() == Some(base))
            })
    })
}

/// `navigator.languages`, most preferred first.
pub fn browser_languages() -> Vec<String> {
    let Some(window) = web_sys::window() else {
        return Vec::new();
    };
    let navigator = js_sys::Reflect::get(&window, &"navigator".into()).unwrap_or(JsValue::NULL);
    let list = js_sys::Reflect::get(&navigator, &"languages".into()).unwrap_or(JsValue::NULL);
    if !js_sys::Array::is_array(&list) {
        return Vec::new();
    }
    list.unchecked_into::<js_sys::Array>()
        .iter()
        .filter_map(|tag| tag.as_string())
        .collect()
}

/// Makes `language` the table, or English for `None`, and sets the page's
/// `lang`. A table that cannot be fetched leaves the current one.
pub async fn load(language: Option<&Language>) -> bool {
    let table = match language {
        None => Some(Table::default()),
        Some(language) => fetch_table(&language.file).await,
    };
    let Some(table) = table else {
        return false;
    };
    TABLE.with(|current| *current.borrow_mut() = table);
    expose();
    if let Some(root) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.document_element())
    {
        let tag = language.map_or("en".to_string(), |language| language.code.replace('_', "-"));
        let _ = root.set_attribute("lang", &tag);
    }
    apply_dom();
    true
}

async fn fetch_table(file: &str) -> Option<Table> {
    let text = fetch_text(&format!("/lang/{file}")).await?;
    let root = js_sys::JSON::parse(&text).ok()?;
    let plural = js_sys::Reflect::get(&root, &"plural".into())
        .ok()?
        .as_string()
        .and_then(|text| Plural::parse(&text));
    let messages = js_sys::Reflect::get(&root, &"messages".into()).ok()?;
    let mut table = Table {
        plural,
        messages: HashMap::new(),
    };
    for entry in js_sys::Object::entries(messages.unchecked_ref::<js_sys::Object>()).iter() {
        let entry: js_sys::Array = entry.unchecked_into();
        let Some(key) = entry.get(0).as_string() else {
            continue;
        };
        let value = entry.get(1);
        let message = if let Some(text) = value.as_string() {
            Message::One(text)
        } else if js_sys::Array::is_array(&value) {
            Message::Forms(
                value
                    .unchecked_into::<js_sys::Array>()
                    .iter()
                    .filter_map(|form| form.as_string())
                    .collect(),
            )
        } else {
            continue;
        };
        table.messages.insert(key, message);
    }
    Some(table)
}

async fn fetch_text(url: &str) -> Option<String> {
    let window = web_sys::window()?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_str(url))
        .await
        .ok()?
        .dyn_into()
        .ok()?;
    if !response.ok() {
        return None;
    }
    JsFuture::from(response.text().ok()?)
        .await
        .ok()?
        .as_string()
}

/// A .po header's `plural=` expression, such as
/// `(n % 10 == 1 && n % 100 != 11 ? 0 : 1)`, in the C subset gettext allows.
struct Plural {
    expression: Expression,
}

enum Expression {
    Number(i64),
    Count,
    Binary(char, Box<Expression>, Box<Expression>),
    Ternary(Box<Expression>, Box<Expression>, Box<Expression>),
}

impl Plural {
    fn parse(header: &str) -> Option<Plural> {
        let source = header.trim();
        let source = source.strip_prefix("plural=").unwrap_or(source);
        let mut parser = Parser {
            tokens: tokens(source)?,
            at: 0,
        };
        let expression = parser.ternary()?;
        (parser.at == parser.tokens.len()).then_some(Plural { expression })
    }

    fn form(&self, count: f64) -> usize {
        let count = count.abs().floor() as i64;
        usize::try_from(evaluate(&self.expression, count)).unwrap_or(0)
    }
}

fn evaluate(expression: &Expression, count: i64) -> i64 {
    match expression {
        Expression::Number(value) => *value,
        Expression::Count => count,
        Expression::Ternary(condition, then, otherwise) => {
            if evaluate(condition, count) != 0 {
                evaluate(then, count)
            } else {
                evaluate(otherwise, count)
            }
        }
        Expression::Binary(operator, left, right) => {
            let left = evaluate(left, count);
            // gettext's operators short-circuit, which only matters for the
            // division by zero a table could not name anyway.
            let right = evaluate(right, count);
            match operator {
                '|' => i64::from(left != 0 || right != 0),
                '&' => i64::from(left != 0 && right != 0),
                '=' => i64::from(left == right),
                '!' => i64::from(left != right),
                '<' => i64::from(left < right),
                '>' => i64::from(left > right),
                'l' => i64::from(left <= right),
                'g' => i64::from(left >= right),
                '+' => left + right,
                '-' => left - right,
                '*' => left * right,
                '/' => left.checked_div(right).unwrap_or(0),
                '%' => left.checked_rem(right).unwrap_or(0),
                _ => 0,
            }
        }
    }
}

enum Token {
    Number(i64),
    Count,
    /// `||` as '|', `&&` as '&', `==` as '=', `!=` as '!', `<=` as 'l',
    /// `>=` as 'g', and the single-character operators as themselves.
    Operator(char),
}

fn tokens(source: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let char = chars[at];
        let next = chars.get(at + 1).copied();
        let pair = match (char, next) {
            ('|', Some('|')) => Some('|'),
            ('&', Some('&')) => Some('&'),
            ('=', Some('=')) => Some('='),
            ('!', Some('=')) => Some('!'),
            ('<', Some('=')) => Some('l'),
            ('>', Some('=')) => Some('g'),
            _ => None,
        };
        if let Some(operator) = pair {
            tokens.push(Token::Operator(operator));
            at += 2;
        } else if char.is_ascii_digit() {
            let end = chars[at..]
                .iter()
                .position(|char| !char.is_ascii_digit())
                .map_or(chars.len(), |length| at + length);
            let text: String = chars[at..end].iter().collect();
            tokens.push(Token::Number(text.parse().ok()?));
            at = end;
        } else if char == 'n' {
            tokens.push(Token::Count);
            at += 1;
        } else if "<>+-*/%?:()".contains(char) {
            tokens.push(Token::Operator(char));
            at += 1;
        } else if char.is_whitespace() || char == ';' {
            at += 1;
        } else {
            return None;
        }
    }
    Some(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

/// Binary operators from loosest to tightest, as in C.
const LEVELS: [&[char]; 6] = [
    &['|'],
    &['&'],
    &['=', '!'],
    &['<', '>', 'l', 'g'],
    &['+', '-'],
    &['*', '/', '%'],
];

impl Parser {
    fn operator(&self) -> Option<char> {
        match self.tokens.get(self.at) {
            Some(Token::Operator(operator)) => Some(*operator),
            _ => None,
        }
    }

    fn ternary(&mut self) -> Option<Expression> {
        let condition = self.binary(0)?;
        if self.operator() != Some('?') {
            return Some(condition);
        }
        self.at += 1;
        let then = self.ternary()?;
        if self.operator() != Some(':') {
            return None;
        }
        self.at += 1;
        let otherwise = self.ternary()?;
        Some(Expression::Ternary(
            Box::new(condition),
            Box::new(then),
            Box::new(otherwise),
        ))
    }

    fn binary(&mut self, level: usize) -> Option<Expression> {
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut left = self.binary(level + 1)?;
        while let Some(operator) = self
            .operator()
            .filter(|operator| LEVELS[level].contains(operator))
        {
            self.at += 1;
            let right = self.binary(level + 1)?;
            left = Expression::Binary(operator, Box::new(left), Box::new(right));
        }
        Some(left)
    }

    fn unary(&mut self) -> Option<Expression> {
        match self.tokens.get(self.at)? {
            Token::Number(value) => {
                self.at += 1;
                Some(Expression::Number(*value))
            }
            Token::Count => {
                self.at += 1;
                Some(Expression::Count)
            }
            Token::Operator('(') => {
                self.at += 1;
                let inner = self.ternary()?;
                if self.operator() != Some(')') {
                    return None;
                }
                self.at += 1;
                Some(inner)
            }
            Token::Operator(_) => None,
        }
    }
}

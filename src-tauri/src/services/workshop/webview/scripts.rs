//! JavaScript snippets injected into the hidden Steam webview.
//!
//! These are pure string builders: they take values in and return JS source,
//! with no access to the webview or any application state. Keeping them apart
//! from SteamWebview makes the in-page logic readable on its own and leaves the
//! struct focused on window and cookie management.

/// Builds a JS snippet that locates the login/password inputs, fills them
/// in via the native `value` setter (so React's onChange handlers fire),
/// and submits the form. This is a near-direct port of
/// `infrastructure/steam/workshop_scripts.py::login_form_fill_script`.
pub(super) fn build_login_script(username: &str, password: &str) -> String {
    let user = escape_js_string(username);
    let pass = escape_js_string(password);
    format!(
        r#"
        (function() {{
            try {{
                // Steam's store login form is React-controlled. Assigning
                // `el.value = x` is accepted by the DOM but React immediately
                // resets it from its own state, so the field ends up empty and
                // the submit does nothing — that is exactly the "I typed my
                // password and nothing happened" bug.
                //
                // The reliable way is to go through the native value setter and
                // then dispatch the events React listens for. React tracks the
                // previous value in `_valueTracker`; if that still holds the old
                // text it treats the change as a no-op, so the tracker has to be
                // reset as well.
                var nativeSetter = Object.getOwnPropertyDescriptor(
                    HTMLInputElement.prototype, 'value'
                ).set;
                function setReactValue(el, v) {{
                    var tracker = el._valueTracker;
                    if (tracker) tracker.setValue('');
                    nativeSetter.call(el, v);
                    el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                    // Some Steam forms listen for key events to enable the
                    // submit button.
                    el.dispatchEvent(new KeyboardEvent('keyup', {{ bubbles: true, key: 'a' }}));
                }}

                var isSearch = function(el) {{
                    var n = (el.getAttribute('name') || '').toLowerCase();
                    var ph = (el.getAttribute('placeholder') || '').toLowerCase();
                    if (n === 'term' || n === 'snr' || n === 'search') return true;
                    if (ph.indexOf('搜索') >= 0 || ph.indexOf('search') >= 0) return true;
                    return false;
                }};
                var isCodeBox = function(el) {{
                    var id = (el.getAttribute('id') || '').toLowerCase();
                    var ph = (el.getAttribute('placeholder') || '').toLowerCase();
                    return id.indexOf('authcode') >= 0 || id.indexOf('twofactor') >= 0
                        || id.indexOf('friendlyname') >= 0
                        || ph.indexOf('验证码') >= 0;
                }};
                var visible = function(el) {{ return el.offsetParent !== null && !el.disabled; }};

                var inputs = Array.prototype.slice.call(document.querySelectorAll('input'));
                // The password field anchors the credential form.
                var pass = null;
                for (var i = 0; i < inputs.length; i++) {{
                    if ((inputs[i].getAttribute('type') || '') === 'password' && visible(inputs[i])) {{
                        pass = inputs[i]; break;
                    }}
                }}
                if (!pass) {{
                    try {{ history.replaceState(null, '', location.pathname + location.search + '#WE_FILL:no-password'); }} catch (e) {{}}
                    return {{ ready: false, why: 'no-password' }};
                }}

                // Username = the visible text input right before the password
                // field in the same form, skipping search / auth-code boxes.
                var scope = pass.form
                    ? Array.prototype.slice.call(pass.form.querySelectorAll('input'))
                    : inputs;
                var pIdx = scope.indexOf(pass);
                var login = null;
                for (var j = pIdx - 1; j >= 0; j--) {{
                    var c = scope[j];
                    var t = (c.getAttribute('type') || '').toLowerCase();
                    if (t !== 'text' && t !== 'email' && t !== 'tel') continue;
                    if (!visible(c) || isSearch(c) || isCodeBox(c)) continue;
                    login = c; break;
                }}
                if (!login) {{
                    try {{ history.replaceState(null, '', location.pathname + location.search + '#WE_FILL:no-username'); }} catch (e) {{}}
                    return {{ ready: false, why: 'no-username' }};
                }}

                setReactValue(login, "{user}");
                setReactValue(pass, "{pass}");

                // Submit: prefer the button whose label is the login action.
                var labelHit = function(el) {{
                    var t = ((el.innerText || el.value || '') + '').trim().toLowerCase();
                    if (!t) return false;
                    return t === '登录' || t === '登入' || t === 'sign in' || t === 'login'
                        || t.indexOf('sign in') >= 0 || t.indexOf('登录') >= 0;
                }};
                var btn = null;
                var containers = pass.form ? [pass.form, document] : [document];
                for (var ci = 0; ci < containers.length && !btn; ci++) {{
                    var cands = Array.prototype.slice.call(
                        containers[ci].querySelectorAll('button, input[type="submit"]')
                    );
                    for (var bi = 0; bi < cands.length; bi++) {{
                        if (cands[bi].disabled || !visible(cands[bi])) continue;
                        if (labelHit(cands[bi])) {{ btn = cands[bi]; break; }}
                    }}
                }}
                if (btn) {{ btn.disabled = false; btn.click(); }}
                try {{
                    history.replaceState(null, '', location.pathname + location.search
                        + '#WE_FILL:ready,clicked=' + (btn ? 1 : 0));
                }} catch (e) {{}}
                return {{ ready: true, clicked: !!btn }};
            }} catch (e) {{
                try {{
                    history.replaceState(null, '', location.pathname + location.search
                        + '#WE_FILL:error');
                }} catch (e2) {{}}
                return {{ error: String(e) }};
            }}
        }})();
        "#
    )
}

/// Classify whatever page the login flow ended up on.
///
/// This exists because the only observable symptom of a failed auto-login was
/// "no cookie", which is indistinguishable between a wrong password, a Steam
/// Guard prompt, a CAPTCHA and a rate-limit block. Each needs a completely
/// different response from the user (and from us), so the page has to be
/// inspected rather than guessed at. Only coarse keyword flags are reported —
/// never page text, never form values.
pub(super) fn build_page_probe_script() -> &'static str {
    r#"
    (function() {
        try {
            var t = ((document.body && document.body.innerText) || '').toLowerCase();
            var title = (document.title || '').toLowerCase();
            var all = t + ' ' + title;
            var flags = [];
            if (all.indexOf('steam guard') >= 0 || all.indexOf('two-factor') >= 0
                || all.indexOf('authenticator') >= 0 || all.indexOf('验证器') >= 0
                || all.indexOf('安全令牌') >= 0) flags.push('GUARD');
            if (all.indexOf('captcha') >= 0 || all.indexOf('验证码') >= 0
                || all.indexOf('人机') >= 0) flags.push('CAPTCHA');
            if (all.indexOf('incorrect') >= 0 || all.indexOf('wrong password') >= 0
                || all.indexOf('密码错误') >= 0 || all.indexOf('密碼錯誤') >= 0
                || all.indexOf('try again') >= 0) flags.push('BADPASS');
            if (all.indexOf('too many') >= 0 || all.indexOf('rate limit') >= 0
                || all.indexOf('频繁') >= 0) flags.push('RATELIMIT');
            if (all.indexOf('sign in') >= 0 || all.indexOf('登录') >= 0
                || all.indexOf('log in') >= 0) flags.push('LOGINPAGE');
            if (location.pathname.indexOf('/login') < 0) flags.push('LEFTLOGIN');
            try {
                history.replaceState(null, '', location.pathname + location.search
                    + '#WE_PAGE:' + (flags.length ? flags.join('+') : 'NONE'));
            } catch (e) {}
        } catch (e) {}
    })();
    "#
}

fn escape_js_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}


use crate::internal::error_pages::ErrorPageData;
use crate::page_data::PageData;
use std::borrow::Cow;
use std::collections::HashMap;
use std::{env, fmt};

/// Escapes special characters in page data that might interfere with JavaScript processing.
fn escape_page_data(data: &str) -> String {
    data.to_string()
        // We escape any backslashes to prevent their interfering with JSON delimiters
        .replace(r#"\"#, r#"\\"#)
        // We escape any backticks, which would interfere with JS's raw strings system
        .replace(r#"`"#, r#"\`"#)
        // We escape any interpolations into JS's raw string system
        .replace(r#"${"#, r#"\${"#)
}

/// Represents a shell of an HTML file. It may have content that gets interpolated into the file.
#[derive(Clone)]
pub struct HtmlShell<'a> {
    /// The actual shell content, on whcih interpolations will be performed.
    shell: String,
    /// Additional contents of the head before the interpolation boundary.
    head_before_boundary: Vec<Cow<'a, str>>,
    /// Scripts to be inserted before the interpolation boundary.
    scripts_before_boundary: Vec<Cow<'a, str>>,
    /// Additional contents of the head after the interpolation boundary. These will be wiped out after a page transition.
    head_after_boundary: Vec<Cow<'a, str>>,
    /// Scripts to be interpolated after the interpolation bounary. These will be wiped out after a page transition.
    scripts_after_boundary: Vec<Cow<'a, str>>,
    /// Content to be interpolated into the body of the shell.
    content: Cow<'a, str>,
    /// The ID of the element into which we'll interpolate content.
    root_id: String,
}
impl<'a> HtmlShell<'a> {
    /// Initializes the HTML shell by interpolating necessary scripts into it and adding the render configuration.
    pub fn new(
        shell: String,
        root_id: &str,
        render_cfg: &HashMap<String, String>,
        path_prefix: &str,
    ) -> Self {
        let mut head_before_boundary = Vec::new();
        let mut scripts_before_boundary = Vec::new();

        // Define the render config as a global variable
        let render_cfg = format!(
            "window.__PERSEUS_RENDER_CFG = '{render_cfg}';",
            // It's safe to assume that something we just deserialized will serialize again in this case
            render_cfg = serde_json::to_string(render_cfg).unwrap()
        );
        scripts_before_boundary.push(render_cfg.into());

        // Inject a global variable to identify whether we are testing (picked up by app shell to trigger helper DOM events)
        if env::var("PERSEUS_TESTING").is_ok() {
            scripts_before_boundary.push("window.__PERSEUS_TESTING = true;".into());
        }

        // Define the script that will load the Wasm bundle (inlined to avoid unnecessary extra requests)
        let load_wasm_bundle = format!(
            r#"
        import init, {{ run }} from "{path_prefix}/.perseus/bundle.js";
        async function main() {{
            await init("{path_prefix}/.perseus/bundle.wasm");
            run();
        }}
        main();
        "#,
            path_prefix = path_prefix
        );
        scripts_before_boundary.push(load_wasm_bundle.into());

        // Add in the `<base>` element at the very top so that it applies to everything in the HTML shell
        // Otherwise any stylesheets loaded before it won't work properly
        //
        // We add a trailing `/` to the base URL (https://stackoverflow.com/a/26043021)
        // Note that it's already had any pre-existing ones stripped away
        let base = format!(r#"<base href="{}/" />"#, path_prefix);
        head_before_boundary.push(base.into());

        Self {
            shell,
            head_before_boundary,
            scripts_before_boundary,
            head_after_boundary: Vec::new(),
            scripts_after_boundary: Vec::new(),
            content: "".into(),
            root_id: root_id.into(),
        }
    }

    /// Interpolates page data into the shell.
    pub fn page_data(mut self, page_data: &'a PageData) -> Self {
        // Interpolate a global variable of the state so the app shell doesn't have to make any more trips
        // The app shell will unset this after usage so it doesn't contaminate later non-initial loads
        // Error pages (above) will set this to `error`
        let initial_state = if let Some(state) = &page_data.state {
            escape_page_data(state)
        } else {
            "None".to_string()
        };

        // We put this at the very end of the head (after the delimiter comment) because it doesn't matter if it's expunged on subsequent loads
        let initial_state = format!("window.__PERSEUS_INITIAL_STATE = `{}`", initial_state);
        self.scripts_after_boundary.push(initial_state.into());
        // Interpolate the document `<head>` (this should of course be removed between page loads)
        self.head_after_boundary.push((&page_data.head).into());
        // And set the content
        self.content = (&page_data.content).into();

        self
    }

    /// Interpolates a fallback for locale redirection pages such that, even if JavaScript is disabled, the user will still be redirected to the default locale.
    /// From there, Perseus' inbuilt progressive enhancement can occur, but without this a user directed to an unlocalized page with JS disabled would see a
    /// blank screen, which is terrible UX. Note that this also includes a fallback for if JS is enabled but Wasm is disabled. Note that the redirect URL
    /// is expected to be generated with a path prefix inbuilt.
    ///
    /// This also adds a `__perseus_initial_state` `<div>` in case it's needed (for Wasm redirections).
    ///
    /// Further, this will preload the Wasm binary, making redirection snappier (but initial load slower), a tradeoff that generally improves UX.
    pub fn locale_redirection_fallback(mut self, redirect_url: &'a str) -> Self {
        // This will be used if JavaScript is completely disabled (it's then the site's responsibility to show a further message)
        let dumb_redirect = format!(
            r#"<noscript>
        <meta http-equiv="refresh" content="0; url={}" />
    </noscript>"#,
            redirect_url
        );

        // This will be used if JS is enabled, but Wasm is disabled or not supported (it's then the site's responsibility to show a further message)
        // Wasm support detector courtesy https://stackoverflow.com/a/47880734
        let js_redirect = format!(
            r#"
        function wasmSupported() {{
            try {{
                if (typeof WebAssembly === "object"
                    && typeof WebAssembly.instantiate === "function") {{
                    const module = new WebAssembly.Module(Uint8Array.of(0x0, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00));
                    if (module instanceof WebAssembly.Module) {{
                        return new WebAssembly.Instance(module) instanceof WebAssembly.Instance;
                    }}
                }}
            }} catch (e) {{}}
            return false;
        }}

        if (!wasmSupported()) {{
            window.location.replace("{}");
        }}
            "#,
            redirect_url
        );

        self.head_after_boundary.push(dumb_redirect.into());
        self.scripts_after_boundary.push(js_redirect.into());
        // TODO Interpolate a preload of the Wasm bundle after the interpolation boundary
        // TODO Do we need any content in here?

        self
    }

    /// Interpolates page error data into the shell in the event of a failure.
    pub fn error_page(mut self, error_page_data: &'a ErrorPageData, error_html: &'a str) -> Self {
        let error = serde_json::to_string(error_page_data).unwrap();
        let state_var = format!(
            "window.__PERSEUS_INITIAL_STATE = `error-{}`;",
            escape_page_data(&error),
        );
        self.scripts_after_boundary.push(state_var.into());
        self.content = error_html.into();

        self
    }
}
// This code actually interpolates everything in the correct places.
impl fmt::Display for HtmlShell<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let head_start = self.head_before_boundary.join("\n");
        // We also inject a delimiter comment that will be used to wall off the constant document head from the interpolated document head
        let head_end = format!(
            r#"
            <script type="module">{scripts_before_boundary}</script>
            <!--PERSEUS_INTERPOLATED_HEAD_BEGINS-->
            {head_after_boundary}
            <script>{scripts_after_boundary}</script>
            "#,
            scripts_before_boundary = self.scripts_before_boundary.join("\n"),
            head_after_boundary = self.head_after_boundary.join("\n"),
            scripts_after_boundary = self.scripts_after_boundary.join("\n"),
        );

        let shell_with_head = self
            .shell
            .replace("<head>", &format!("<head>{}", head_start))
            .replace("</head>", &format!("{}</head>", head_end));

        // The user MUST place have a `<div>` of this exact form (documented explicitly)
        // We permit either double or single quotes
        let html_to_replace_double = format!("<div id=\"{}\">", self.root_id);
        let html_to_replace_single = format!("<div id='{}'>", self.root_id);
        let html_replacement = format!(
            // We give the content a specific ID so that it can be deleted if an error page needs to be rendered on the client-side
            r#"{}<div id="__perseus_content_initial" class="__perseus_content">{}</div>"#,
            &html_to_replace_double, self.content,
        );
        // Now interpolate that HTML into the HTML shell
        let new_shell = shell_with_head
            .replace(&html_to_replace_double, &html_replacement)
            .replace(&html_to_replace_single, &html_replacement);

        f.write_str(&new_shell)
    }
}

#[cfg(test)]
mod tests {
    use crate::{internal::error_pages::ErrorPageData, page_data::PageData};
    use std::{collections::HashMap, iter::FromIterator};

    use super::HtmlShell;

    const SHELL: &str = r#"
    <html>
        <head>
            <title>Shell</title>
        </head>
        <body>
            <p>Content</p>
            <div id="root_id"></div>
        </body>
    </html>
    "#;

    fn get_render_config() -> HashMap<String, String> {
        HashMap::from_iter([("key".into(), "value".into())])
    }

    #[test]
    fn basic_shell() {
        let shell = HtmlShell::new(SHELL.into(), "root_id", &get_render_config(), "prefix");
        println!("{}", shell);
    }

    #[test]
    fn page_data_shell() {
        let page_data = PageData {
            content: "page_data.content".to_string(),
            state: Some("page_data.state".to_string()),
            head: "page_data.head".to_string(),
        };

        let shell = HtmlShell::new(SHELL.into(), "root_id", &get_render_config(), "prefix")
            .page_data(&page_data);

        println!("{}", shell);
    }

    #[test]
    fn redirect_fallback_shell() {
        let shell = HtmlShell::new(SHELL.into(), "root_id", &get_render_config(), "prefix")
            .locale_redirection_fallback("redirect_url");

        println!("{}", shell);
    }

    #[test]
    fn error_page_shell() {
        let error_page_data = ErrorPageData {
            url: "Made up URL".to_string(),
            status: 404,
            err: "page not found",
        };

        let shell = HtmlShell::new(SHELL.into(), "root_id", &get_render_config(), "prefix")
            .error_page(&error_page_data, "Page not found.");

        println!("{}", shell);
    }
}

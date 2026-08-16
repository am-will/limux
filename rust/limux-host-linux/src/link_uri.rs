fn scheme_and_rest(url: &str) -> Option<(String, &str)> {
    let colon = url.find(':')?;
    Some((url[..colon].to_ascii_lowercase(), &url[colon..]))
}

pub fn is_safe_external_url(url: &str) -> bool {
    let Some((scheme, rest)) = scheme_and_rest(url) else {
        return false;
    };

    match scheme.as_str() {
        "http" | "https" => rest.starts_with("://"),
        "mailto" => rest.starts_with(':'),
        _ => false,
    }
}

pub fn is_embedded_browser_url(url: &str) -> bool {
    let Some((scheme, rest)) = scheme_and_rest(url) else {
        return false;
    };

    matches!(scheme.as_str(), "http" | "https") && rest.starts_with("://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_urls_allow_web_and_email_schemes() {
        for url in [
            "https://example.com",
            "HTTP://localhost:8080/path",
            "mailto:user@example.com",
            "MAILTO:user@example.com?subject=hi",
        ] {
            assert!(is_safe_external_url(url), "should accept {url}");
        }
    }

    #[test]
    fn embedded_urls_allow_web_schemes_only() {
        for url in ["https://example.com", "HTTP://localhost:8080/path"] {
            assert!(is_embedded_browser_url(url), "should accept {url}");
        }
        assert!(!is_embedded_browser_url("mailto:user@example.com"));
    }

    #[test]
    fn link_urls_reject_unsafe_schemes_and_malformed_values() {
        for url in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "data:text/html,hello",
            "vbscript:msgbox(1)",
            "file:///etc/passwd",
            "ftp://ftp.example.com/file",
            "ftps://ftp.example.com/file",
            "smb://host/share",
            "nfs://host/export",
            "dav://host/path",
            "davs://host/path",
            "sftp://host/path",
            "ssh://host/path",
            "magnet:?xt=urn:btih:abc",
            "chrome://settings",
            "about:blank",
            "vscode://file/etc/passwd",
            "slack://open?team=T",
            " example.com",
            "/etc/passwd",
            "example.com",
            "",
            "https:",
            "https:example.com",
            "http:/example.com",
        ] {
            assert!(!is_safe_external_url(url), "should reject {url}");
            assert!(!is_embedded_browser_url(url), "should reject {url}");
        }
    }
}

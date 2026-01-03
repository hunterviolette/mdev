pub fn language_hint_for_path(path: &str) -> &str {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "rs" => "rs",
        "ts" => "ts",
        "tsx" => "tsx",
        "js" => "js",
        "jsx" => "jsx",
        "json" => "json",
        "yml" => "yml",
        "yaml" => "yaml",
        "toml" => "toml",
        "md" => "md",
        "html" => "html",
        "htm" => "htm",
        "css" => "css",
        "scss" => "scss",
        "py" => "py",
        "go" => "go",
        "java" => "java",
        "kt" => "kt",
        "kts" => "kts",
        "c" => "c",
        "h" => "h",
        "cpp" | "cc" | "cxx" => "cpp",
        "hpp" | "hh" | "hxx" => "hpp",
        "cs" => "cs",
        "sh" => "sh",
        "ps1" => "ps1",
        "sql" => "sql",
        "xml" => "xml",
        _ => "txt",
    }
}

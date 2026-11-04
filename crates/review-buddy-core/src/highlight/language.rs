//! Lightweight, table-driven language descriptions for the incremental lexer.
//! Delimiters are ASCII byte strings; the lexer works on UTF-8 bytes.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::LazyLock;

/// FNV-1a: fast for the short identifiers we look up on every token.
pub struct FnvHasher(u64);

impl Default for FnvHasher {
    fn default() -> Self {
        FnvHasher(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for FnvHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut h = self.0;
        for &b in bytes {
            h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
        self.0 = h;
    }
}

/// Allocation-free word lookup by byte slice.
#[derive(Clone, Debug, Default)]
pub struct WordSet(HashSet<&'static [u8], BuildHasherDefault<FnvHasher>>);

impl WordSet {
    pub fn new(words: &[&'static str]) -> Self {
        WordSet(words.iter().map(|w| w.as_bytes()).collect())
    }
    #[inline]
    pub fn contains(&self, word: &[u8]) -> bool {
        !self.0.is_empty() && self.0.contains(word)
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct LanguageSpec {
    pub name: &'static str,
    pub line_comments: &'static [&'static str],
    pub block_comments: &'static [(&'static str, &'static str)],
    /// Single-line string delimiters, e.g. `"` and `'` (ASCII bytes).
    pub string_delimiters: &'static [u8],
    /// Delimiters that may span lines, e.g. `"""`, `` ` ``, `'''`.
    pub multiline_strings: &'static [(&'static str, &'static str)],
    pub keywords: WordSet,
    pub constants: WordSet,
    pub builtin_types: WordSet,
    /// Identifiers starting with an uppercase letter are types.
    pub capitalized_are_types: bool,
    /// `@decorator` / `#directive` prefix characters (ASCII bytes).
    pub attribute_prefixes: &'static [u8],
    /// Markup languages: `<tag attr="">`.
    pub is_markup: bool,
    /// JSON/YAML/TOML: `key:` / `key =` highlighted as property.
    pub key_value: bool,
}

/// Static description used to build a `LanguageSpec` (struct-update friendly).
struct Def {
    name: &'static str,
    line: &'static [&'static str],
    block: &'static [(&'static str, &'static str)],
    strings: &'static str,
    multiline: &'static [(&'static str, &'static str)],
    keywords: &'static [&'static str],
    constants: &'static [&'static str],
    types: &'static [&'static str],
    capitalized_are_types: bool,
    attributes: &'static str,
    is_markup: bool,
    key_value: bool,
}

impl Def {
    const fn new(name: &'static str) -> Def {
        Def {
            name,
            line: &[],
            block: &[],
            strings: "\"'",
            multiline: &[],
            keywords: &[],
            constants: &[],
            types: &[],
            capitalized_are_types: true,
            attributes: "",
            is_markup: false,
            key_value: false,
        }
    }

    fn build(self) -> LanguageSpec {
        LanguageSpec {
            name: self.name,
            line_comments: self.line,
            block_comments: self.block,
            string_delimiters: self.strings.as_bytes(),
            multiline_strings: self.multiline,
            keywords: WordSet::new(self.keywords),
            constants: WordSet::new(self.constants),
            builtin_types: WordSet::new(self.types),
            capitalized_are_types: self.capitalized_are_types,
            attribute_prefixes: self.attributes.as_bytes(),
            is_markup: self.is_markup,
            key_value: self.key_value,
        }
    }
}

const C_FAMILY_BLOCK: &[(&str, &str)] = &[("/*", "*/")];

pub static PLAIN: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def { strings: "", capitalized_are_types: false, ..Def::new("Text") }.build()
});

pub static SWIFT: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def {
        line: &["//"],
        block: C_FAMILY_BLOCK,
        strings: "\"",
        multiline: &[("\"\"\"", "\"\"\"")],
        keywords: &[
            "associatedtype", "class", "deinit", "enum", "extension", "fileprivate", "func", "import", "init",
            "inout", "internal", "let", "open", "operator", "private", "precedencegroup", "protocol", "public",
            "rethrows", "static", "struct", "subscript", "typealias", "var", "break", "case", "catch", "continue",
            "default", "defer", "do", "else", "fallthrough", "for", "guard", "if", "in", "repeat", "return",
            "throw", "switch", "where", "while", "as", "is", "try", "await", "async", "throws", "some", "any",
            "final", "override", "mutating", "nonmutating", "lazy", "weak", "unowned", "convenience", "required",
            "indirect", "actor", "nonisolated", "isolated", "consuming", "borrowing", "sending", "package", "macro",
            "get", "set", "willSet", "didSet",
        ],
        constants: &["true", "false", "nil", "self", "Self", "super"],
        attributes: "@#",
        ..Def::new("Swift")
    }
    .build()
});

pub static TYPESCRIPT: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def {
        line: &["//"],
        block: C_FAMILY_BLOCK,
        strings: "\"'",
        multiline: &[("`", "`")],
        keywords: &[
            "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do",
            "else", "export", "extends", "finally", "for", "function", "if", "import", "in", "instanceof",
            "new", "return", "switch", "throw", "try", "typeof", "var", "void", "while", "with", "yield",
            "let", "static", "enum", "await", "async", "implements", "package", "protected", "interface",
            "private", "public", "type", "from", "as", "of", "readonly", "declare", "namespace", "abstract",
            "keyof", "satisfies", "infer", "is", "get", "set", "module",
        ],
        constants: &["true", "false", "null", "undefined", "this", "super", "NaN", "Infinity"],
        types: &["string", "number", "boolean", "any", "unknown", "never", "object", "symbol", "bigint"],
        attributes: "@",
        ..Def::new("TypeScript")
    }
    .build()
});

pub static PYTHON: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def {
        line: &["#"],
        strings: "\"'",
        multiline: &[("\"\"\"", "\"\"\""), ("'''", "'''")],
        keywords: &[
            "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else",
            "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal",
            "not", "or", "pass", "raise", "return", "try", "while", "with", "yield", "match", "case",
        ],
        constants: &["True", "False", "None", "self", "cls"],
        types: &["int", "str", "float", "bool", "list", "dict", "set", "tuple", "bytes", "object"],
        attributes: "@",
        ..Def::new("Python")
    }
    .build()
});

pub static GO: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def {
        line: &["//"],
        block: C_FAMILY_BLOCK,
        strings: "\"'",
        multiline: &[("`", "`")],
        keywords: &[
            "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for",
            "func", "go", "goto", "if", "import", "interface", "map", "package", "range", "return", "select",
            "struct", "switch", "type", "var",
        ],
        constants: &["true", "false", "nil", "iota"],
        types: &[
            "bool", "byte", "complex64", "complex128", "error", "float32", "float64", "int", "int8", "int16",
            "int32", "int64", "rune", "string", "uint", "uint8", "uint16", "uint32", "uint64", "uintptr", "any",
        ],
        ..Def::new("Go")
    }
    .build()
});

pub static RUST: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def {
        line: &["//"],
        block: C_FAMILY_BLOCK,
        strings: "\"",
        keywords: &[
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
            "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
            "return", "static", "struct", "super", "trait", "type", "unsafe", "use", "where", "while",
        ],
        constants: &["true", "false", "self", "Self", "None", "Some", "Ok", "Err"],
        types: &[
            "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32",
            "f64", "bool", "char", "str", "String", "Vec", "Option", "Result", "Box",
        ],
        attributes: "#",
        ..Def::new("Rust")
    }
    .build()
});

pub static C_LIKE: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def {
        line: &["//"],
        block: C_FAMILY_BLOCK,
        strings: "\"'",
        keywords: &[
            "auto", "break", "case", "const", "continue", "default", "do", "else", "enum", "extern", "for",
            "goto", "if", "inline", "register", "restrict", "return", "sizeof", "static", "struct", "switch",
            "typedef", "union", "volatile", "while", "class", "namespace", "template", "typename", "public",
            "private", "protected", "virtual", "override", "new", "delete", "using", "constexpr", "noexcept",
            "try", "catch", "throw", "operator", "friend", "explicit", "mutable", "final", "import",
            "interface", "implementation", "end", "property", "nonatomic", "strong", "weak", "readonly",
        ],
        constants: &["true", "false", "NULL", "nullptr", "nil", "YES", "NO", "this", "self"],
        types: &[
            "void", "char", "short", "int", "long", "float", "double", "signed", "unsigned", "bool", "size_t",
            "int8_t", "int16_t", "int32_t", "int64_t", "uint8_t", "uint16_t", "uint32_t", "uint64_t", "id", "auto",
        ],
        attributes: "#@",
        ..Def::new("C")
    }
    .build()
});

pub static JAVA: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def {
        line: &["//"],
        block: C_FAMILY_BLOCK,
        strings: "\"'",
        multiline: &[("\"\"\"", "\"\"\"")],
        keywords: &[
            "abstract", "assert", "break", "case", "catch", "class", "const", "continue", "default", "do",
            "else", "enum", "extends", "final", "finally", "for", "goto", "if", "implements", "import",
            "instanceof", "interface", "native", "new", "package", "private", "protected", "public", "return",
            "static", "strictfp", "super", "switch", "synchronized", "throw", "throws", "transient", "try",
            "volatile", "while", "var", "record", "sealed", "permits", "yield",
            // Kotlin
            "fun", "val", "when", "object", "companion", "data", "open", "override", "internal", "lateinit",
            "suspend", "is", "in", "out", "by", "init", "typealias", "inline", "reified",
            // C#
            "namespace", "using", "readonly", "async", "await", "get", "set", "struct", "virtual", "sealed",
            "partial", "ref", "params", "foreach", "lock", "operator", "event", "delegate",
        ],
        constants: &["true", "false", "null", "this", "super", "it"],
        types: &[
            "int", "long", "short", "byte", "char", "float", "double", "boolean", "void", "string", "bool",
            "decimal", "object", "Unit", "Int", "Long", "Boolean", "String", "Any",
        ],
        attributes: "@",
        ..Def::new("Java")
    }
    .build()
});

pub static RUBY: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def {
        line: &["#"],
        block: &[("=begin", "=end")],
        strings: "\"'",
        keywords: &[
            "alias", "and", "begin", "break", "case", "class", "def", "defined?", "do", "else", "elsif", "end",
            "ensure", "for", "if", "in", "module", "next", "not", "or", "redo", "rescue", "retry", "return",
            "super", "then", "undef", "unless", "until", "when", "while", "yield", "require",
            "require_relative", "attr_reader", "attr_accessor", "attr_writer", "private", "protected", "public",
        ],
        constants: &["true", "false", "nil", "self"],
        attributes: "@:",
        ..Def::new("Ruby")
    }
    .build()
});

pub static SHELL: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def {
        line: &["#"],
        strings: "\"'",
        keywords: &[
            "if", "then", "else", "elif", "fi", "case", "esac", "for", "while", "until", "do", "done", "in",
            "function", "return", "local", "export", "readonly", "declare", "set", "unset", "shift", "source",
            "echo", "exit", "cd", "FROM", "RUN", "CMD", "COPY", "ADD", "ENV", "ARG", "WORKDIR", "EXPOSE",
            "ENTRYPOINT", "USER", "VOLUME", "LABEL",
        ],
        constants: &["true", "false"],
        capitalized_are_types: false,
        attributes: "$",
        ..Def::new("Shell")
    }
    .build()
});

pub static JSON: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def {
        line: &["//"],
        strings: "\"",
        constants: &["true", "false", "null"],
        capitalized_are_types: false,
        key_value: true,
        ..Def::new("JSON")
    }
    .build()
});

pub static YAML: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def {
        line: &["#"],
        strings: "\"'",
        constants: &["true", "false", "null", "yes", "no", "on", "off", "~"],
        capitalized_are_types: false,
        attributes: "&*",
        key_value: true,
        ..Def::new("YAML")
    }
    .build()
});

pub static TOML: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def {
        line: &["#"],
        strings: "\"'",
        multiline: &[("\"\"\"", "\"\"\"")],
        constants: &["true", "false"],
        capitalized_are_types: false,
        key_value: true,
        ..Def::new("TOML")
    }
    .build()
});

pub static CSS: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def {
        block: C_FAMILY_BLOCK,
        strings: "\"'",
        keywords: &["import", "media", "keyframes", "font-face", "supports", "important", "from", "to"],
        capitalized_are_types: false,
        attributes: "@$",
        ..Def::new("CSS")
    }
    .build()
});

pub static MARKUP: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def {
        block: &[("<!--", "-->")],
        strings: "\"'",
        capitalized_are_types: false,
        is_markup: true,
        ..Def::new("HTML")
    }
    .build()
});

pub static SQL: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def {
        line: &["--"],
        block: C_FAMILY_BLOCK,
        strings: "'\"",
        keywords: &[
            "select", "from", "where", "insert", "into", "values", "update", "set", "delete", "create", "table",
            "alter", "drop", "index", "join", "left", "right", "inner", "outer", "on", "and", "or", "not",
            "null", "as", "group", "by", "order", "having", "limit", "offset", "primary", "key", "foreign",
            "references", "default", "unique", "begin", "commit", "rollback", "union", "all", "distinct",
            "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE",
            "TABLE", "ALTER", "DROP", "INDEX", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "ON", "AND", "OR",
            "NOT", "NULL", "AS", "GROUP", "BY", "ORDER", "HAVING", "LIMIT", "OFFSET", "PRIMARY", "KEY",
            "FOREIGN", "REFERENCES", "DEFAULT", "UNIQUE", "BEGIN", "COMMIT", "UNION", "ALL", "DISTINCT",
        ],
        capitalized_are_types: false,
        ..Def::new("SQL")
    }
    .build()
});

pub static PHP: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def {
        line: &["//", "#"],
        block: C_FAMILY_BLOCK,
        strings: "\"'",
        keywords: &[
            "abstract", "and", "as", "break", "case", "catch", "class", "const", "continue", "declare",
            "default", "do", "echo", "else", "elseif", "extends", "final", "finally", "fn", "for", "foreach",
            "function", "global", "if", "implements", "include", "interface", "match", "namespace", "new",
            "or", "private", "protected", "public", "require", "return", "static", "switch", "throw", "trait",
            "try", "use", "var", "while", "yield",
        ],
        constants: &["true", "false", "null", "this", "self", "parent"],
        attributes: "$@",
        ..Def::new("PHP")
    }
    .build()
});

pub static MARKDOWN: LazyLock<LanguageSpec> = LazyLock::new(|| {
    Def { strings: "`", capitalized_are_types: false, attributes: "#", ..Def::new("Markdown") }.build()
});

static BY_EXTENSION: LazyLock<HashMap<&'static str, &'static LanguageSpec>> = LazyLock::new(|| {
    let table: [(&'static LanguageSpec, &[&'static str]); 17] = [
        (&SWIFT, &["swift"]),
        (&TYPESCRIPT, &["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "vue", "svelte"]),
        (&PYTHON, &["py", "pyi", "pyw"]),
        (&GO, &["go"]),
        (&RUST, &["rs"]),
        (&C_LIKE, &["c", "h", "cc", "cpp", "cxx", "hpp", "hh", "m", "mm", "metal", "cu", "proto"]),
        (&JAVA, &["java", "kt", "kts", "scala", "groovy", "gradle", "cs", "dart"]),
        (&RUBY, &["rb", "rake", "gemspec", "podspec"]),
        (&SHELL, &["sh", "bash", "zsh", "fish", "mk"]),
        (&JSON, &["json", "jsonc", "json5", "geojson"]),
        (&YAML, &["yml", "yaml"]),
        (&TOML, &["toml", "ini", "cfg", "conf", "properties", "env"]),
        (&CSS, &["css", "scss", "sass", "less"]),
        (&MARKUP, &["html", "htm", "xml", "svg", "plist", "xib", "storyboard", "xaml", "csproj"]),
        (&SQL, &["sql"]),
        (&PHP, &["php"]),
        (&MARKDOWN, &["md", "markdown", "mdx"]),
    ];
    let mut m = HashMap::new();
    for (spec, exts) in table {
        for e in exts {
            m.insert(*e, spec);
        }
    }
    m
});

pub struct LanguageRegistry;

impl LanguageRegistry {
    pub fn language_for_path(path: &str) -> &'static LanguageSpec {
        let last = path.split('/').rfind(|s| !s.is_empty()).unwrap_or(path);
        let name = last.to_lowercase();
        if name == "dockerfile" || name.starts_with("dockerfile.") {
            return &SHELL;
        }
        if name == "makefile" || name == "gnumakefile" {
            return &SHELL;
        }
        if matches!(name.as_str(), "podfile" | "gemfile" | "rakefile" | "fastfile") {
            return &RUBY;
        }
        let Some(dot) = name.rfind('.') else { return &PLAIN };
        BY_EXTENSION.get(&name[dot + 1..]).copied().unwrap_or(&PLAIN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_by_path() {
        assert_eq!(LanguageRegistry::language_for_path("a/b/c.tsx").name, "TypeScript");
        assert_eq!(LanguageRegistry::language_for_path("Dockerfile").name, "Shell");
        assert_eq!(LanguageRegistry::language_for_path("README").name, "Text");
        assert_eq!(LanguageRegistry::language_for_path("x.YML").name, "YAML");
        assert_eq!(LanguageRegistry::language_for_path("ios/Podfile").name, "Ruby");
        assert_eq!(LanguageRegistry::language_for_path("src/main.rs").name, "Rust");
        assert_eq!(LanguageRegistry::language_for_path("weird.").name, "Text");
    }

    #[test]
    fn word_set_lookup() {
        assert!(SWIFT.keywords.contains(b"func"));
        assert!(!SWIFT.keywords.contains(b"fun"));
        assert!(PLAIN.keywords.is_empty());
    }
}

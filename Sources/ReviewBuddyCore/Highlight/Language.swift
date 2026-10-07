import Foundation

/// Lightweight, table-driven language description for the incremental lexer.
public struct LanguageSpec: Sendable {
    public var name: String
    public var lineComments: [[UInt16]]
    public var blockComments: [([UInt16], [UInt16])]
    /// Single-line string delimiters, e.g. `"` and `'`.
    public var stringDelimiters: [UInt16]
    /// Delimiters that may span lines, e.g. `"""`, `` ` ``, `'''`.
    public var multilineStrings: [([UInt16], [UInt16])]
    public var keywords: HashedWordSet
    public var constants: HashedWordSet
    public var builtinTypes: HashedWordSet
    /// Identifiers starting with an uppercase letter are types.
    public var capitalizedAreTypes: Bool
    /// `@decorator` / `#directive` prefix characters.
    public var attributePrefixes: [UInt16]
    /// Markup languages: `<tag attr="">`.
    public var isMarkup: Bool
    /// JSON/YAML/TOML: `key:` / `key =` highlighted as property.
    public var keyValue: Bool

    init(name: String, line: [String] = [], block: [(String, String)] = [], strings: String = "\"'",
         multiline: [(String, String)] = [], keywords: [String] = [], constants: [String] = [],
         types: [String] = [], capitalizedAreTypes: Bool = true, attributes: String = "",
         isMarkup: Bool = false, keyValue: Bool = false) {
        self.name = name
        self.lineComments = line.map { Array($0.utf16) }
        self.blockComments = block.map { (Array($0.0.utf16), Array($0.1.utf16)) }
        self.stringDelimiters = Array(strings.utf16)
        self.multilineStrings = multiline.map { (Array($0.0.utf16), Array($0.1.utf16)) }
        self.keywords = HashedWordSet(keywords)
        self.constants = HashedWordSet(constants)
        self.builtinTypes = HashedWordSet(types)
        self.capitalizedAreTypes = capitalizedAreTypes
        self.attributePrefixes = Array(attributes.utf16)
        self.isMarkup = isMarkup
        self.keyValue = keyValue
    }

    public static let plain = LanguageSpec(name: "Text", strings: "", capitalizedAreTypes: false)
}

/// Allocation-free word lookup: FNV-1a hash of UTF-16 units → set membership.
public struct HashedWordSet: Sendable {
    private var hashes: Set<UInt64>
    public init(_ words: [String]) { hashes = Set(words.map { Self.hash(Array($0.utf16)[...]) }) }
    @inline(__always) public static func hash(_ s: ArraySlice<UInt16>) -> UInt64 {
        var h: UInt64 = 0xcbf29ce484222325
        for c in s { h = (h ^ UInt64(c)) &* 0x100000001b3 }
        return h
    }
    @inline(__always) public func contains(hash: UInt64) -> Bool { hashes.contains(hash) }
    public var isEmpty: Bool { hashes.isEmpty }
}

public enum LanguageRegistry {
    public static func language(forPath path: String) -> LanguageSpec {
        let name = (path.split(separator: "/").last.map(String.init) ?? path).lowercased()
        if name == "dockerfile" || name.hasPrefix("dockerfile.") { return shell }
        if name == "makefile" || name == "gnumakefile" { return shell }
        if name == "podfile" || name == "gemfile" || name == "rakefile" || name == "fastfile" { return ruby }
        guard let dot = name.lastIndex(of: ".") else { return .plain }
        let ext = String(name[name.index(after: dot)...])
        return byExtension[ext] ?? .plain
    }

    static let cFamilyBlock = [("/*", "*/")]

    public static let swift = LanguageSpec(
        name: "Swift", line: ["//"], block: cFamilyBlock, strings: "\"", multiline: [("\"\"\"", "\"\"\"")],
        keywords: ["associatedtype", "class", "deinit", "enum", "extension", "fileprivate", "func", "import", "init",
                   "inout", "internal", "let", "open", "operator", "private", "precedencegroup", "protocol", "public",
                   "rethrows", "static", "struct", "subscript", "typealias", "var", "break", "case", "catch", "continue",
                   "default", "defer", "do", "else", "fallthrough", "for", "guard", "if", "in", "repeat", "return",
                   "throw", "switch", "where", "while", "as", "is", "try", "await", "async", "throws", "some", "any",
                   "final", "override", "mutating", "nonmutating", "lazy", "weak", "unowned", "convenience", "required",
                   "indirect", "actor", "nonisolated", "isolated", "consuming", "borrowing", "sending", "package", "macro",
                   "get", "set", "willSet", "didSet"],
        constants: ["true", "false", "nil", "self", "Self", "super"],
        attributes: "@#")

    public static let typescript = LanguageSpec(
        name: "TypeScript", line: ["//"], block: cFamilyBlock, strings: "\"'", multiline: [("`", "`")],
        keywords: ["break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do",
                   "else", "export", "extends", "finally", "for", "function", "if", "import", "in", "instanceof",
                   "new", "return", "switch", "throw", "try", "typeof", "var", "void", "while", "with", "yield",
                   "let", "static", "enum", "await", "async", "implements", "package", "protected", "interface",
                   "private", "public", "type", "from", "as", "of", "readonly", "declare", "namespace", "abstract",
                   "keyof", "satisfies", "infer", "is", "get", "set", "module"],
        constants: ["true", "false", "null", "undefined", "this", "super", "NaN", "Infinity"],
        types: ["string", "number", "boolean", "any", "unknown", "never", "object", "symbol", "bigint"],
        attributes: "@")

    public static let python = LanguageSpec(
        name: "Python", line: ["#"], strings: "\"'", multiline: [("\"\"\"", "\"\"\""), ("'''", "'''")],
        keywords: ["and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else",
                   "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal",
                   "not", "or", "pass", "raise", "return", "try", "while", "with", "yield", "match", "case"],
        constants: ["True", "False", "None", "self", "cls"],
        types: ["int", "str", "float", "bool", "list", "dict", "set", "tuple", "bytes", "object"],
        attributes: "@")

    public static let go = LanguageSpec(
        name: "Go", line: ["//"], block: cFamilyBlock, strings: "\"'", multiline: [("`", "`")],
        keywords: ["break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for",
                   "func", "go", "goto", "if", "import", "interface", "map", "package", "range", "return", "select",
                   "struct", "switch", "type", "var"],
        constants: ["true", "false", "nil", "iota"],
        types: ["bool", "byte", "complex64", "complex128", "error", "float32", "float64", "int", "int8", "int16",
                "int32", "int64", "rune", "string", "uint", "uint8", "uint16", "uint32", "uint64", "uintptr", "any"])

    public static let rust = LanguageSpec(
        name: "Rust", line: ["//"], block: cFamilyBlock, strings: "\"",
        keywords: ["as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
                   "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
                   "return", "static", "struct", "super", "trait", "type", "unsafe", "use", "where", "while"],
        constants: ["true", "false", "self", "Self", "None", "Some", "Ok", "Err"],
        types: ["i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32",
                "f64", "bool", "char", "str", "String", "Vec", "Option", "Result", "Box"],
        attributes: "#")

    public static let cLike = LanguageSpec(
        name: "C", line: ["//"], block: cFamilyBlock, strings: "\"'",
        keywords: ["auto", "break", "case", "const", "continue", "default", "do", "else", "enum", "extern", "for",
                   "goto", "if", "inline", "register", "restrict", "return", "sizeof", "static", "struct", "switch",
                   "typedef", "union", "volatile", "while", "class", "namespace", "template", "typename", "public",
                   "private", "protected", "virtual", "override", "new", "delete", "using", "constexpr", "noexcept",
                   "try", "catch", "throw", "operator", "friend", "explicit", "mutable", "final", "import",
                   "interface", "implementation", "end", "property", "nonatomic", "strong", "weak", "readonly"],
        constants: ["true", "false", "NULL", "nullptr", "nil", "YES", "NO", "this", "self"],
        types: ["void", "char", "short", "int", "long", "float", "double", "signed", "unsigned", "bool", "size_t",
                "int8_t", "int16_t", "int32_t", "int64_t", "uint8_t", "uint16_t", "uint32_t", "uint64_t", "id", "auto"],
        attributes: "#@")

    public static let java = LanguageSpec(
        name: "Java", line: ["//"], block: cFamilyBlock, strings: "\"'", multiline: [("\"\"\"", "\"\"\"")],
        keywords: ["abstract", "assert", "break", "case", "catch", "class", "const", "continue", "default", "do",
                   "else", "enum", "extends", "final", "finally", "for", "goto", "if", "implements", "import",
                   "instanceof", "interface", "native", "new", "package", "private", "protected", "public", "return",
                   "static", "strictfp", "super", "switch", "synchronized", "throw", "throws", "transient", "try",
                   "volatile", "while", "var", "record", "sealed", "permits", "yield",
                   // Kotlin
                   "fun", "val", "when", "object", "companion", "data", "open", "override", "internal", "lateinit",
                   "suspend", "is", "in", "out", "by", "init", "typealias", "inline", "reified",
                   // C#
                   "namespace", "using", "readonly", "async", "await", "get", "set", "struct", "virtual", "sealed",
                   "partial", "ref", "params", "foreach", "lock", "operator", "event", "delegate"],
        constants: ["true", "false", "null", "this", "super", "it"],
        types: ["int", "long", "short", "byte", "char", "float", "double", "boolean", "void", "string", "bool",
                "decimal", "object", "Unit", "Int", "Long", "Boolean", "String", "Any"],
        attributes: "@")

    public static let ruby = LanguageSpec(
        name: "Ruby", line: ["#"], block: [("=begin", "=end")], strings: "\"'",
        keywords: ["alias", "and", "begin", "break", "case", "class", "def", "defined?", "do", "else", "elsif", "end",
                   "ensure", "for", "if", "in", "module", "next", "not", "or", "redo", "rescue", "retry", "return",
                   "super", "then", "undef", "unless", "until", "when", "while", "yield", "require",
                   "require_relative", "attr_reader", "attr_accessor", "attr_writer", "private", "protected", "public"],
        constants: ["true", "false", "nil", "self"],
        attributes: "@:")

    public static let shell = LanguageSpec(
        name: "Shell", line: ["#"], strings: "\"'",
        keywords: ["if", "then", "else", "elif", "fi", "case", "esac", "for", "while", "until", "do", "done", "in",
                   "function", "return", "local", "export", "readonly", "declare", "set", "unset", "shift", "source",
                   "echo", "exit", "cd", "FROM", "RUN", "CMD", "COPY", "ADD", "ENV", "ARG", "WORKDIR", "EXPOSE",
                   "ENTRYPOINT", "USER", "VOLUME", "LABEL"],
        constants: ["true", "false"], capitalizedAreTypes: false, attributes: "$")

    public static let json = LanguageSpec(
        name: "JSON", line: ["//"], strings: "\"", constants: ["true", "false", "null"], capitalizedAreTypes: false,
        keyValue: true)

    public static let yaml = LanguageSpec(
        name: "YAML", line: ["#"], strings: "\"'", constants: ["true", "false", "null", "yes", "no", "on", "off", "~"],
        capitalizedAreTypes: false, attributes: "&*", keyValue: true)

    public static let toml = LanguageSpec(
        name: "TOML", line: ["#"], strings: "\"'", multiline: [("\"\"\"", "\"\"\"")],
        constants: ["true", "false"], capitalizedAreTypes: false, keyValue: true)

    public static let css = LanguageSpec(
        name: "CSS", block: cFamilyBlock, strings: "\"'",
        keywords: ["import", "media", "keyframes", "font-face", "supports", "important", "from", "to"],
        capitalizedAreTypes: false, attributes: "@$")

    public static let markup = LanguageSpec(
        name: "HTML", block: [("<!--", "-->")], strings: "\"'", capitalizedAreTypes: false, isMarkup: true)

    public static let sql = LanguageSpec(
        name: "SQL", line: ["--"], block: cFamilyBlock, strings: "'\"",
        keywords: ["select", "from", "where", "insert", "into", "values", "update", "set", "delete", "create", "table",
                   "alter", "drop", "index", "join", "left", "right", "inner", "outer", "on", "and", "or", "not",
                   "null", "as", "group", "by", "order", "having", "limit", "offset", "primary", "key", "foreign",
                   "references", "default", "unique", "begin", "commit", "rollback", "union", "all", "distinct",
                   "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE",
                   "TABLE", "ALTER", "DROP", "INDEX", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "ON", "AND", "OR",
                   "NOT", "NULL", "AS", "GROUP", "BY", "ORDER", "HAVING", "LIMIT", "OFFSET", "PRIMARY", "KEY",
                   "FOREIGN", "REFERENCES", "DEFAULT", "UNIQUE", "BEGIN", "COMMIT", "UNION", "ALL", "DISTINCT"],
        capitalizedAreTypes: false)

    public static let php = LanguageSpec(
        name: "PHP", line: ["//", "#"], block: cFamilyBlock, strings: "\"'",
        keywords: ["abstract", "and", "as", "break", "case", "catch", "class", "const", "continue", "declare",
                   "default", "do", "echo", "else", "elseif", "extends", "final", "finally", "fn", "for", "foreach",
                   "function", "global", "if", "implements", "include", "interface", "match", "namespace", "new",
                   "or", "private", "protected", "public", "require", "return", "static", "switch", "throw", "trait",
                   "try", "use", "var", "while", "yield"],
        constants: ["true", "false", "null", "this", "self", "parent"], attributes: "$@")

    public static let markdown = LanguageSpec(
        name: "Markdown", strings: "`", capitalizedAreTypes: false, attributes: "#")

    static let byExtension: [String: LanguageSpec] = {
        var m: [String: LanguageSpec] = [:]
        func add(_ spec: LanguageSpec, _ exts: [String]) { for e in exts { m[e] = spec } }
        add(swift, ["swift"])
        add(typescript, ["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "vue", "svelte"])
        add(python, ["py", "pyi", "pyw"])
        add(go, ["go"])
        add(rust, ["rs"])
        add(cLike, ["c", "h", "cc", "cpp", "cxx", "hpp", "hh", "m", "mm", "metal", "cu", "proto"])
        add(java, ["java", "kt", "kts", "scala", "groovy", "gradle", "cs", "dart"])
        add(ruby, ["rb", "rake", "gemspec", "podspec"])
        add(shell, ["sh", "bash", "zsh", "fish", "mk"])
        add(json, ["json", "jsonc", "json5", "geojson"])
        add(yaml, ["yml", "yaml"])
        add(toml, ["toml", "ini", "cfg", "conf", "properties", "env"])
        add(css, ["css", "scss", "sass", "less"])
        add(markup, ["html", "htm", "xml", "svg", "plist", "xib", "storyboard", "xaml", "csproj"])
        add(sql, ["sql"])
        add(php, ["php"])
        add(markdown, ["md", "markdown", "mdx"])
        return m
    }()
}

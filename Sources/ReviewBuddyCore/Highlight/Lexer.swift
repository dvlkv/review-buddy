import Foundation

public enum TokenKind: UInt8, Sendable, CaseIterable {
    case plain, keyword, type, string, number, comment, function, attribute, constant, tag, property
}

/// A highlighted span in UTF-16 units (directly usable with NSAttributedString).
public struct SyntaxToken: Sendable, Equatable {
    public var start: UInt32
    public var length: UInt32
    public var kind: TokenKind
    public init(start: Int, length: Int, kind: TokenKind) {
        self.start = UInt32(start); self.length = UInt32(length); self.kind = kind
    }
}

/// Lexer state carried between consecutive lines.
public enum LexState: Equatable, Sendable {
    case normal
    case blockComment(Int)
    case multilineString(Int)
}

/// A fast single-pass, line-at-a-time lexer. It is not a parser — it only needs
/// to be right about comments, strings, keywords, numbers and types, which is what
/// matters visually in a diff. Roughly 50–100 MB/s on typical source.
public struct Lexer: Sendable {
    public let spec: LanguageSpec

    public init(spec: LanguageSpec) { self.spec = spec }

    public func tokenize(_ line: String, state: inout LexState) -> [SyntaxToken] {
        let u = Array(line.utf16)
        return tokenize(utf16: u, state: &state)
    }

    public func tokenize(utf16 u: [UInt16], state: inout LexState) -> [SyntaxToken] {
        var tokens: [SyntaxToken] = []
        let n = u.count
        var i = 0

        @inline(__always) func matches(_ pat: [UInt16], at p: Int) -> Bool {
            guard p + pat.count <= n else { return false }
            for k in 0..<pat.count where u[p + k] != pat[k] { return false }
            return true
        }
        func find(_ pat: [UInt16], from p: Int) -> Int? {
            var q = p
            while q + pat.count <= n {
                if u[q] == pat[0] && matches(pat, at: q) { return q }
                q += 1
            }
            return nil
        }

        // Continue a construct from the previous line.
        switch state {
        case .blockComment(let idx):
            let close = spec.blockComments[idx].1
            if let e = find(close, from: 0) {
                tokens.append(SyntaxToken(start: 0, length: e + close.count, kind: .comment))
                i = e + close.count
                state = .normal
            } else {
                if n > 0 { tokens.append(SyntaxToken(start: 0, length: n, kind: .comment)) }
                return tokens
            }
        case .multilineString(let idx):
            let close = spec.multilineStrings[idx].1
            if let e = findStringEnd(u, close, from: 0) {
                tokens.append(SyntaxToken(start: 0, length: e + close.count, kind: .string))
                i = e + close.count
                state = .normal
            } else {
                if n > 0 { tokens.append(SyntaxToken(start: 0, length: n, kind: .string)) }
                return tokens
            }
        case .normal:
            break
        }

        if spec.isMarkup { return tokenizeMarkup(u, from: i, state: &state, tokens: tokens) }

        var lastWasDot = false
        lineLoop: while i < n {
            let c = u[i]
            // Whitespace
            if c == 32 || c == 9 { i += 1; continue }

            // Line comments
            for lc in spec.lineComments where u[i] == lc[0] && matches(lc, at: i) {
                // Shell-like `#` only counts at token start (avoid `$#`, URLs with `#` fragment in YAML values ok).
                tokens.append(SyntaxToken(start: i, length: n - i, kind: .comment))
                break lineLoop
            }
            // Block comments
            for (bi, bc) in spec.blockComments.enumerated() where u[i] == bc.0[0] && matches(bc.0, at: i) {
                if let e = find(bc.1, from: i + bc.0.count) {
                    tokens.append(SyntaxToken(start: i, length: e + bc.1.count - i, kind: .comment))
                    i = e + bc.1.count
                } else {
                    tokens.append(SyntaxToken(start: i, length: n - i, kind: .comment))
                    state = .blockComment(bi)
                    break lineLoop
                }
                continue lineLoop
            }
            // Multiline strings (checked before single-line so `"""` wins over `"`).
            for (mi, ms) in spec.multilineStrings.enumerated() where u[i] == ms.0[0] && matches(ms.0, at: i) {
                if let e = findStringEnd(u, ms.1, from: i + ms.0.count) {
                    tokens.append(SyntaxToken(start: i, length: e + ms.1.count - i, kind: .string))
                    i = e + ms.1.count
                } else {
                    tokens.append(SyntaxToken(start: i, length: n - i, kind: .string))
                    state = .multilineString(mi)
                    break lineLoop
                }
                continue lineLoop
            }
            // Strings
            if spec.stringDelimiters.contains(c) {
                let end = findStringEnd(u, [c], from: i + 1) ?? (n - 1)
                let len = end + 1 - i
                // Key-value languages: `"key":` is a property.
                var kind: TokenKind = .string
                if spec.keyValue {
                    var j = end + 1
                    while j < n, u[j] == 32 { j += 1 }
                    if j < n, u[j] == 58 /* : */ { kind = .property }
                }
                tokens.append(SyntaxToken(start: i, length: min(len, n - i), kind: kind))
                i = end + 1
                lastWasDot = false
                continue
            }
            // Numbers
            if isDigit(c) || (c == 46 && i + 1 < n && isDigit(u[i + 1]) && !lastWasDot) {
                let s = i
                i += 1
                while i < n, isIdent(u[i]) || u[i] == 46 && i + 1 < n && isDigit(u[i + 1]) { i += 1 }
                tokens.append(SyntaxToken(start: s, length: i - s, kind: .number))
                lastWasDot = false
                continue
            }
            // Attributes / decorators / directives / variables
            if spec.attributePrefixes.contains(c), i + 1 < n, isIdentStart(u[i + 1]) || u[i + 1] == 123 /* { */ {
                let s = i
                i += 1
                if u[i] == 123 { while i < n, u[i] != 125 { i += 1 }; i = min(n, i + 1) }
                else { while i < n, isIdent(u[i]) || u[i] == 45 && spec.name == "CSS" { i += 1 } }
                tokens.append(SyntaxToken(start: s, length: i - s, kind: .attribute))
                lastWasDot = false
                continue
            }
            // Identifiers
            if isIdentStart(c) {
                let s = i
                i += 1
                while i < n, isIdent(u[i]) { i += 1 }
                let h = HashedWordSet.hash(u[s..<i])
                var kind: TokenKind = .plain
                if !lastWasDot && spec.keywords.contains(hash: h) {
                    kind = .keyword
                } else if spec.constants.contains(hash: h) {
                    kind = .constant
                } else if spec.builtinTypes.contains(hash: h) {
                    kind = .type
                } else if spec.capitalizedAreTypes && c >= 65 && c <= 90 {
                    kind = .type
                } else {
                    var j = i
                    while j < n, u[j] == 32 { j += 1 }
                    if j < n && u[j] == 40 /* ( */ && !spec.keyValue {
                        kind = .function
                    } else if spec.keyValue && j < n && (u[j] == 58 || u[j] == 61) && (tokens.isEmpty || isLeadingKey(u, s)) {
                        kind = .property
                    }
                }
                if kind != .plain { tokens.append(SyntaxToken(start: s, length: i - s, kind: kind)) }
                lastWasDot = false
                continue
            }
            lastWasDot = (c == 46)
            i += 1
        }
        return tokens
    }

    /// Finds the closing delimiter, honoring backslash escapes.
    private func findStringEnd(_ u: [UInt16], _ close: [UInt16], from p: Int) -> Int? {
        let n = u.count
        var q = p
        while q < n {
            if u[q] == 92 /* \ */ { q += 2; continue }
            if u[q] == close[0] {
                var ok = q + close.count <= n
                if ok { for k in 1..<close.count where u[q + k] != close[k] { ok = false; break } }
                if ok { return q }
            }
            q += 1
        }
        return nil
    }

    private func isLeadingKey(_ u: [UInt16], _ s: Int) -> Bool {
        var j = s - 1
        while j >= 0, u[j] == 32 || u[j] == 9 || u[j] == 45 /* - */ { j -= 1 }
        return j < 0
    }

    private func tokenizeMarkup(_ u: [UInt16], from start: Int, state: inout LexState, tokens initial: [SyntaxToken]) -> [SyntaxToken] {
        var tokens = initial
        let n = u.count
        var i = start
        let open = spec.blockComments.first?.0 ?? []
        let close = spec.blockComments.first?.1 ?? []
        while i < n {
            if !open.isEmpty && u[i] == 60 && i + open.count <= n && Array(u[i..<i + open.count]) == open {
                var q = i + open.count
                var found = false
                while q + close.count <= n { if Array(u[q..<q + close.count]) == close { found = true; break }; q += 1 }
                if found {
                    tokens.append(SyntaxToken(start: i, length: q + close.count - i, kind: .comment))
                    i = q + close.count
                } else {
                    tokens.append(SyntaxToken(start: i, length: n - i, kind: .comment))
                    state = .blockComment(0)
                    return tokens
                }
                continue
            }
            if u[i] == 60 /* < */ {
                var j = i + 1
                if j < n && (u[j] == 47 || u[j] == 33 || u[j] == 63) { j += 1 }
                let s = j
                while j < n, isIdent(u[j]) || u[j] == 45 || u[j] == 58 || u[j] == 46 { j += 1 }
                if j > s { tokens.append(SyntaxToken(start: s, length: j - s, kind: .tag)) }
                i = j
                // Attributes until '>'
                while i < n && u[i] != 62 {
                    if u[i] == 34 || u[i] == 39 {
                        let q = u[i]
                        var e = i + 1
                        while e < n && u[e] != q { e += 1 }
                        tokens.append(SyntaxToken(start: i, length: min(n, e + 1) - i, kind: .string))
                        i = e + 1
                    } else if isIdentStart(u[i]) {
                        let a = i
                        while i < n, isIdent(u[i]) || u[i] == 45 || u[i] == 58 { i += 1 }
                        tokens.append(SyntaxToken(start: a, length: i - a, kind: .attribute))
                    } else { i += 1 }
                }
                continue
            }
            i += 1
        }
        return tokens
    }

    @inline(__always) private func isDigit(_ c: UInt16) -> Bool { c >= 48 && c <= 57 }
    @inline(__always) private func isIdentStart(_ c: UInt16) -> Bool {
        (c >= 65 && c <= 90) || (c >= 97 && c <= 122) || c == 95 || c > 127
    }
    @inline(__always) private func isIdent(_ c: UInt16) -> Bool { isIdentStart(c) || isDigit(c) || c == 63 && spec.name == "Ruby" }
}

/// Highlights all lines of a file diff, tracking lexer state separately for the
/// old and new sides so block comments that span lines render correctly.
public enum DiffHighlighter {
    /// Returns tokens for each line in `file.lines` (same indexing).
    public static func highlight(file: FileDiff, in diff: DiffSet) -> [[SyntaxToken]] {
        let lexer = Lexer(spec: LanguageRegistry.language(forPath: file.path))
        var result = [[SyntaxToken]](repeating: [], count: file.lines.count)
        if lexer.spec.name == "Text" { return result }
        for hunk in file.hunks {
            var oldState = LexState.normal
            var newState = LexState.normal
            for i in hunk.lineRange {
                let line = file.lines[i]
                let text = diff.text(of: line)
                switch line.kind {
                case .context:
                    var s = newState
                    result[i] = lexer.tokenize(text, state: &s)
                    newState = s
                    oldState = s
                case .addition:
                    result[i] = lexer.tokenize(text, state: &newState)
                case .deletion:
                    result[i] = lexer.tokenize(text, state: &oldState)
                case .noNewline:
                    break
                }
            }
        }
        return result
    }
}

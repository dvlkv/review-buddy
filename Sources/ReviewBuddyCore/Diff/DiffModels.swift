import Foundation

/// Which side of a diff a line or anchor belongs to. Matches GitHub's `DiffSide`.
public enum DiffSide: String, Codable, Sendable, Hashable {
    /// Old version (GitHub `LEFT`).
    case left = "LEFT"
    /// New version (GitHub `RIGHT`).
    case right = "RIGHT"
}

public enum LineKind: UInt8, Sendable {
    case context
    case addition
    case deletion
    /// `\ No newline at end of file`
    case noNewline
}

/// A single line of a parsed diff. Text is stored as a byte range into the
/// owning `DiffSet.buffer` so a 200k-line diff costs no per-line allocations.
public struct DiffLine: Sendable, Equatable {
    public var kind: LineKind
    /// 1-based line number in the old file, 0 when the line has none.
    public var oldLine: Int32
    /// 1-based line number in the new file, 0 when the line has none.
    public var newLine: Int32
    /// Byte offset of the text (after the `+`/`-`/` ` marker).
    public var textStart: Int32
    public var textLength: Int32

    public init(kind: LineKind, oldLine: Int32, newLine: Int32, textStart: Int32, textLength: Int32) {
        self.kind = kind
        self.oldLine = oldLine
        self.newLine = newLine
        self.textStart = textStart
        self.textLength = textLength
    }

    public func lineNumber(on side: DiffSide) -> Int {
        Int(side == .left ? oldLine : newLine)
    }
}

public struct Hunk: Sendable, Equatable {
    public var oldStart: Int
    public var oldCount: Int
    public var newStart: Int
    public var newCount: Int
    /// Text after the closing `@@` (usually the enclosing function signature).
    public var heading: String
    /// Range into `FileDiff.lines`.
    public var lineRange: Range<Int>

    public init(oldStart: Int, oldCount: Int, newStart: Int, newCount: Int, heading: String, lineRange: Range<Int>) {
        self.oldStart = oldStart
        self.oldCount = oldCount
        self.newStart = newStart
        self.newCount = newCount
        self.heading = heading
        self.lineRange = lineRange
    }

    public var headerText: String {
        var s = "@@ -\(oldStart),\(oldCount) +\(newStart),\(newCount) @@"
        if !heading.isEmpty { s += " " + heading }
        return s
    }
}

public enum FileStatus: String, Sendable, Codable {
    case added, deleted, modified, renamed, copied
}

public struct FileDiff: Sendable, Identifiable {
    public var id: String { path }
    public var oldPath: String
    public var newPath: String
    public var status: FileStatus
    public var isBinary: Bool
    public var hunks: [Hunk]
    public var lines: ContiguousArray<DiffLine>
    public var additions: Int
    public var deletions: Int

    public init(oldPath: String, newPath: String, status: FileStatus, isBinary: Bool = false,
                hunks: [Hunk] = [], lines: ContiguousArray<DiffLine> = [], additions: Int = 0, deletions: Int = 0) {
        self.oldPath = oldPath
        self.newPath = newPath
        self.status = status
        self.isBinary = isBinary
        self.hunks = hunks
        self.lines = lines
        self.additions = additions
        self.deletions = deletions
    }

    /// The path GitHub uses to address the file (the new path, or the old one for deletions).
    public var path: String { status == .deleted ? oldPath : newPath }
    public var changedLineCount: Int { additions + deletions }
}

/// A fully parsed multi-file diff that owns the raw bytes all lines point into.
public final class DiffSet: @unchecked Sendable {
    public let buffer: [UInt8]
    public private(set) var files: [FileDiff]
    private let indexByPath: [String: Int]
    /// Per-file "generated" flag, computed once (layout rebuilds are frequent).
    public let isGenerated: [Bool]

    public init(buffer: [UInt8], files: [FileDiff]) {
        self.buffer = buffer
        self.files = files
        var index: [String: Int] = [:]
        index.reserveCapacity(files.count * 2)
        for (i, f) in files.enumerated() {
            index[f.newPath] = i
            if index[f.oldPath] == nil { index[f.oldPath] = i }
        }
        self.indexByPath = index
        self.isGenerated = files.map { GeneratedFileDetector.isGenerated($0.path) }
    }

    public func fileIndex(path: String) -> Int? { indexByPath[path] }

    public func text(of line: DiffLine) -> String {
        let start = Int(line.textStart)
        let end = start + Int(line.textLength)
        return buffer.withUnsafeBufferPointer { ptr in
            String(decoding: UnsafeBufferPointer(rebasing: ptr[start..<end]), as: UTF8.self)
        }
    }

    public func text(file: Int, line: Int) -> String {
        text(of: files[file].lines[line])
    }

    public var totalLines: Int { files.reduce(0) { $0 + $1.lines.count } }
}

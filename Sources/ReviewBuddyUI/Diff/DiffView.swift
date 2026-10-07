import AppKit
import SwiftUI
import ReviewBuddyCore

/// SwiftUI wrapper around the AppKit diff canvas.
public struct DiffView: NSViewRepresentable {
    let store: PRStore

    public init(store: PRStore) { self.store = store }

    public func makeCoordinator() -> DiffTableCoordinator { DiffTableCoordinator(store: store) }

    public func makeNSView(context: Context) -> NSScrollView {
        let c = context.coordinator
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = false
        scroll.autohidesScrollers = true
        scroll.drawsBackground = true
        scroll.backgroundColor = Theme.canvas
        scroll.contentView.postsBoundsChangedNotifications = true
        scroll.postsFrameChangedNotifications = true

        let canvas = DiffCanvasView()
        canvas.coordinator = c
        canvas.autoresizingMask = [.width]
        scroll.documentView = canvas

        let sticky = StickyHeaderView()
        sticky.store = store
        sticky.isHidden = true
        scroll.addSubview(sticky)

        c.canvas = canvas
        c.scrollView = scroll
        c.sticky = sticky
        let nc = NotificationCenter.default
        nc.addObserver(c, selector: #selector(DiffTableCoordinator.boundsChanged(_:)),
                       name: NSView.boundsDidChangeNotification, object: scroll.contentView)
        nc.addObserver(c, selector: #selector(DiffTableCoordinator.frameChanged(_:)),
                       name: NSView.frameDidChangeNotification, object: scroll)
        store.view = c
        DispatchQueue.main.async {
            c.layoutDidChange(preserveScroll: false)
            c.focusDiff()
        }
        return scroll
    }

    public func updateNSView(_ nsView: NSScrollView, context: Context) {
        let c = context.coordinator
        if c.store !== store {
            c.store = store
            c.sticky?.store = store
            store.view = c
            c.layoutDidChange(preserveScroll: false)
        }
    }

    /// Fill whatever space SwiftUI offers. Without this, SwiftUI may size the scroll
    /// view to its document's fitting height (the full diff), stretching the window.
    public func sizeThatFits(_ proposal: ProposedViewSize, nsView: NSScrollView, context: Context) -> CGSize? {
        CGSize(width: proposal.width ?? 600, height: proposal.height ?? 400)
    }

    public static func dismantleNSView(_ nsView: NSScrollView, coordinator: DiffTableCoordinator) {
        NotificationCenter.default.removeObserver(coordinator)
    }
}

extension DiffTableCoordinator {
    @objc func boundsChanged(_ note: Notification) { viewportDidScroll() }
    @objc func frameChanged(_ note: Notification) {
        guard let canvas, let sv = scrollView else { return }
        if abs(canvas.frame.width - sv.contentView.bounds.width) > 0.5 {
            canvas.setFrameSize(CGSize(width: sv.contentView.bounds.width, height: canvas.frame.height))
        }
        updateViewport(force: true)
    }
}

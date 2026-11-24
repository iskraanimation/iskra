// Prints the quick guide to a PDF on macOS, through WebKit: each `.page` of the HTML is one
// A4 page, its text kept as text.
//
//     swift docs/guide/make-pdf.swift
//
// It reads docs/guide/quick-guide.html and writes docs/Iskra-Quick-Guide.pdf, both beside
// this script's folder; run it from anywhere.

import AppKit
import PDFKit
import WebKit

/// A page as the HTML lays it out, in CSS pixels: A4 at 96 to the inch.
let pageSize = CGSize(width: 794, height: 1123)
/// From CSS pixels to PDF points: A4 is 595 by 842 points.
let zoom: CGFloat = 0.75

let here = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let root = here.deletingLastPathComponent().deletingLastPathComponent()
let html = here.appendingPathComponent("quick-guide.html")
let out = here.deletingLastPathComponent().appendingPathComponent("Iskra-Quick-Guide.pdf")

final class Printer: NSObject, WKNavigationDelegate {
    let web: WKWebView

    override init() {
        let config = WKWebViewConfiguration()
        web = WKWebView(frame: CGRect(origin: .zero, size: CGSize(width: pageSize.width * zoom, height: pageSize.height * zoom)), configuration: config)
        super.init()
        web.pageZoom = zoom
        web.navigationDelegate = self
    }

    func webView(_ web: WKWebView, didFinish _: WKNavigation!) {
        // The fonts first, then the count of pages.
        web.evaluateJavaScript("document.fonts.ready.then(() => document.querySelectorAll('.page').length)") { _, _ in
            web.evaluateJavaScript("document.querySelectorAll('.page').length") { count, error in
                guard let count = count as? Int, count > 0 else {
                    Swift.print("no pages: \(String(describing: error))")
                    exit(1)
                }
                let tall = CGFloat(count) * pageSize.height * zoom
                web.frame = CGRect(x: 0, y: 0, width: pageSize.width * zoom, height: tall)
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { self.render(page: 0, of: count, into: PDFDocument()) }
            }
        }
    }

    func render(page: Int, of count: Int, into document: PDFDocument) {
        if page == count {
            document.write(to: out)
            Swift.print("wrote \(count) pages to \(out.path)")
            exit(0)
        }
        let config = WKPDFConfiguration()
        config.rect = CGRect(x: 0, y: CGFloat(page) * pageSize.height * zoom, width: pageSize.width * zoom, height: pageSize.height * zoom)
        web.createPDF(configuration: config) { result in
            switch result {
            case .success(let data):
                if let made = PDFDocument(data: data), let first = made.page(at: 0) {
                    document.insert(first, at: document.pageCount)
                }
                self.render(page: page + 1, of: count, into: document)
            case .failure(let error):
                Swift.print("page \(page + 1): \(error)")
                exit(1)
            }
        }
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
let printer = Printer()
printer.web.loadFileURL(html, allowingReadAccessTo: root)
app.run()

import AppKit
import WebKit
import ImageIO
import UniformTypeIdentifiers
import CryptoKit

let args = CommandLine.arguments
let mode = args.count > 1 ? args[1] : "auto"
let port = args.count > 2 ? args[2] : "18765"
let kind = args.count > 3 ? args[3] : "animated"
let directory = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)

func generateGIF(_ animated: Bool) {
    let url = directory.appendingPathComponent(animated ? "animated.gif" : "static.gif")
    let count = animated ? 8 : 1
    let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.gif.identifier as CFString, count, nil)!
    CGImageDestinationSetProperties(dest, [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFLoopCount: 0]] as CFDictionary)
    for frame in 0..<count {
        let context = CGContext(data: nil, width: 256, height: 256, bitsPerComponent: 8, bytesPerRow: 1024, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
        context.setFillColor(CGColor(red: CGFloat(frame % 3) / 3, green: CGFloat(frame % 5) / 5, blue: 0.7, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: 256, height: 256))
        for index in 0..<64 {
            context.setFillColor(CGColor(red: CGFloat((index + frame) % 7) / 7, green: CGFloat(index % 5) / 5, blue: CGFloat(frame) / 8, alpha: 1))
            context.fill(CGRect(x: (index * 173 + frame * 31) % 256, y: (index * 79) % 256, width: 32, height: 32))
        }
        CGImageDestinationAddImage(dest, context.makeImage()!, [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFDelayTime: 0.08]] as CFDictionary)
    }
    precondition(CGImageDestinationFinalize(dest))
}

if mode == "generate" {
    generateGIF(true)
    generateGIF(false)
    exit(0)
}

final class Probe: NSObject, NSApplicationDelegate, WKScriptMessageHandler {
    var window: NSWindow!
    var webview: WKWebView!
    func snapshot(_ suffix: String) {
        webview.takeSnapshot(with: nil) { image, error in
            guard let image, let tiff = image.tiffRepresentation,
                  let bitmap = NSBitmapImageRep(data: tiff), let png = bitmap.representation(using: .png, properties: [:]) else { return }
            let name = "\(mode)-\(kind)-animation-\(suffix).png"
            try? png.write(to: directory.appendingPathComponent(name))
            let hash = SHA256.hash(data: png).map { String(format: "%02x", $0) }.joined()
            print("{\"stage\":\"animation-snapshot\",\"mode\":\"\(mode)\",\"kind\":\"\(kind)\",\"sample\":\"\(suffix)\",\"sha256\":\"\(hash)\"}")
            fflush(stdout)
            if suffix == "a" { DispatchQueue.main.asyncAfter(deadline: .now() + 0.22) { self.snapshot("b") } }
        }
    }
    func applicationDidFinishLaunching(_ notification: Notification) {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.userContentController.add(self, name: "probe")
        webview = WKWebView(frame: NSRect(x: 0, y: 0, width: 900, height: 650), configuration: configuration)
        window = NSWindow(contentRect: webview.frame, styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = "Isolated image decoder investigation: \(mode) / \(kind)"
        window.contentView = webview
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        let html = """
        <!doctype html><html><body style="background:#eee"><div id="status"></div><div id="images" style="display:flex;flex-wrap:wrap"></div><script>
        const mode = '\(mode)', kind = '\(kind)';
        const sleep = ms => new Promise(r => setTimeout(r, ms));
        const report = stage => { const info = {stage, mode, kind, dom: document.images.length, loaded: Array.from(document.images).filter(img => img.complete && img.naturalWidth > 0).length, visibility: document.visibilityState}; document.querySelector('#status').textContent = JSON.stringify(info); window.webkit.messageHandlers.probe.postMessage(info); };
        (async () => {
          report('baseline'); await sleep(5000);
          for (let batch = 0; batch < 3; batch++) {
            const container = document.querySelector('#images');
            for (let i = 0; i < 50; i++) {
              const img = new Image(); img.width = 80; img.height = 80;
              img.decoding = mode; img.src = 'http://127.0.0.1:\(port)/' + kind + '.gif?image=' + (batch * 50 + i);
              container.append(img);
            }
            await sleep(4500); report('loaded-' + batch); await sleep(3500);
            container.replaceChildren(); report('removed-' + batch); await sleep(3500);
          }
          report('idle-start'); await sleep(15000); report('idle-end'); await sleep(4000);
          window.webkit.messageHandlers.probe.postMessage({stage:'finished',mode,kind});
        })();
        </script></body></html>
        """
        webview.loadHTMLString(html, baseURL: URL(string: "http://127.0.0.1:\(port)/"))
        DispatchQueue.main.asyncAfter(deadline: .now() + 90) { print("{\"stage\":\"timeout\"}"); fflush(stdout); exit(2) }
    }
    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        if let data = try? JSONSerialization.data(withJSONObject: message.body, options: [.sortedKeys]), let text = String(data: data, encoding: .utf8) { print(text); fflush(stdout) }
        if let value = message.body as? [String: Any] {
            if value["stage"] as? String == "loaded-0" { snapshot("a") }
            if value["stage"] as? String == "finished" { exit(0) }
        }
    }
}
let app = NSApplication.shared
let probe = Probe()
app.setActivationPolicy(.accessory)
app.delegate = probe
app.run()

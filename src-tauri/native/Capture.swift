import AppKit
import ApplicationServices

// A separate native process keeps blocked accessibility providers away from the UI.
let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
var enabled = false
var dragging = false
var lastTrust: Bool?
var suspended = false
var excluded: [String] = []
var last = ""
var observedPID: pid_t = 0
var observer: AXObserver?
func output(_ object: [String: Any]) {
    guard JSONSerialization.isValidJSONObject(object), let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]), let line = String(data: data, encoding: .utf8), line != last else { return }
    last = line
    print(line); fflush(stdout)
}
func value(_ element: AXUIElement, _ key: String) -> CFTypeRef? {
    var result: CFTypeRef?
    return AXUIElementCopyAttributeValue(element, key as CFString, &result) == .success ? result : nil
}
func poll(settled: Bool = false) {
    let trusted = AXIsProcessTrusted()
    if lastTrust != trusted { lastTrust = trusted; output(["kind":"status","trusted":trusted]) }
    guard trusted else { return }
    guard enabled && !suspended && !dragging else { output(["kind":"clear"]); return }
    guard let front = NSWorkspace.shared.frontmostApplication else { output(["kind":"clear"]); return }
    let identifier = front.bundleIdentifier ?? front.localizedName ?? "unknown"
    if front.processIdentifier == getpid() || identifier == "app.claro.translate" { output(["kind":"clear","reason":"self"]); return }
    if excluded.contains(where: { $0.caseInsensitiveCompare(identifier) == .orderedSame }) { output(["kind":"clear"]); return }
    if front.processIdentifier != observedPID {
        if let old = observer { CFRunLoopRemoveSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(old), .commonModes) }
        observedPID = front.processIdentifier
        let element = AXUIElementCreateApplication(observedPID)
        var next: AXObserver?
        if AXObserverCreate(observedPID, { _,_,_,_ in poll() }, &next) == .success, let next {
            observer = next
            for event in [kAXSelectedTextChangedNotification, kAXFocusedUIElementChangedNotification] {
                AXObserverAddNotification(next, element, event as CFString, nil)
            }
            CFRunLoopAddSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(next), .commonModes)
        }
    }
    let root = AXUIElementCreateApplication(front.processIdentifier)
    AXUIElementSetMessagingTimeout(root, 0.15)
    guard let focusedRef = value(root, kAXFocusedUIElementAttribute), CFGetTypeID(focusedRef) == AXUIElementGetTypeID() else { output(["kind":"clear"]); return }
    let focused = unsafeBitCast(focusedRef, to: AXUIElement.self)
    let role = value(focused,kAXRoleAttribute) as? String ?? ""
    let subrole = value(focused,kAXSubroleAttribute) as? String ?? ""
    guard !subrole.lowercased().contains("secure"), !role.lowercased().contains("password"),
          let text = value(focused,kAXSelectedTextAttribute) as? String, !text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty,
          text.utf8.count <= 12000 else { output(["kind":"clear"]); return }
    let pointer = NSEvent.mouseLocation
    let screenHeight = NSScreen.screens.first?.frame.height ?? 0
    var rect = CGRect(x:pointer.x, y:screenHeight-pointer.y, width:1, height:1)
    if let range = value(focused,kAXSelectedTextRangeAttribute) {
        var bounds: CFTypeRef?
        if AXUIElementCopyParameterizedAttributeValue(focused,kAXBoundsForRangeParameterizedAttribute as CFString,range,&bounds) == .success,
           let bounds, CFGetTypeID(bounds) == AXValueGetTypeID() {
            var candidate = CGRect.zero
            if AXValueGetValue(unsafeBitCast(bounds,to:AXValue.self), .cgRect, &candidate), !candidate.isEmpty { rect = candidate }
        }
    }
    output(["kind":"selection","settled":settled,"text":text,"app":identifier,"x":rect.minX,"y":rect.maxY,"top":rect.minY,"width":rect.width])
}
let _ = NSEvent.addGlobalMonitorForEvents(matching:[.leftMouseDown,.rightMouseDown,.keyDown]) { event in
    if event.type == .keyDown && event.keyCode != 53 { return }
    if event.type != .keyDown {
      let mouse = NSEvent.mouseLocation
      let point = CGPoint(x:mouse.x,y:(NSScreen.screens.first?.frame.height ?? 0)-mouse.y)
      let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly,.excludeDesktopElements],kCGNullWindowID) as? [[String:Any]] ?? []
      for window in windows {
        guard let bounds=window[kCGWindowBounds as String] as? [String:Any], let rect=CGRect(dictionaryRepresentation:bounds as CFDictionary),rect.contains(point) else { continue }
        if let pid=window[kCGWindowOwnerPID as String] as? Int, pid == Int(CommandLine.arguments.dropFirst().first ?? "0") { return }
        break
      }
    }
    if event.type == .leftMouseDown { dragging = true }
    output(["kind":"dismiss"])
}
let _ = NSEvent.addGlobalMonitorForEvents(matching:[.leftMouseUp,.keyUp]) { event in
 if event.type == .leftMouseUp { output(["kind":"pointer-up"]); dragging = false; DispatchQueue.main.asyncAfter(deadline:.now()+0.06) { poll(settled:true) } }
 else if (123...126).contains(Int(event.keyCode)) || event.modifierFlags.contains(.shift) { poll() }
}
NSWorkspace.shared.notificationCenter.addObserver(forName:NSWorkspace.didActivateApplicationNotification,object:nil,queue:.main) { _ in let own = NSWorkspace.shared.frontmostApplication?.bundleIdentifier == "app.claro.translate"; output(["kind":"clear","reason":own ? "self" : "app-switch"]); poll() }
NSWorkspace.shared.notificationCenter.addObserver(forName:NSWorkspace.sessionDidResignActiveNotification,object:nil,queue:.main) { _ in suspended = true; output(["kind":"clear"]) }
NSWorkspace.shared.notificationCenter.addObserver(forName:NSWorkspace.sessionDidBecomeActiveNotification,object:nil,queue:.main) { _ in suspended = false; poll() }
DistributedNotificationCenter.default().addObserver(forName:NSNotification.Name("com.apple.screenIsLocked"),object:nil,queue:.main) { _ in suspended = true; output(["kind":"clear"]) }
DistributedNotificationCenter.default().addObserver(forName:NSNotification.Name("com.apple.screenIsUnlocked"),object:nil,queue:.main) { _ in suspended = false; poll() }
DispatchQueue.global().async {
 while let line = readLine() {
  let fields=line.components(separatedBy:"\t")
  DispatchQueue.main.async {
    if line == "P" { let _ = AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String:true] as CFDictionary); return }
    enabled = fields.first == "1"; excluded = Array(fields.dropFirst()).filter { !$0.isEmpty }; poll()
  }
 }
 exit(0)
}
Timer.scheduledTimer(withTimeInterval:0.6,repeats:true) { _ in poll() }
poll()
app.run()

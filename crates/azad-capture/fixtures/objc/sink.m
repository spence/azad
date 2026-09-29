// Foreground key sink: records every key event AppKit delivers to its window as JSON lines.
// Usage: open -n Sink.app --args <output.jsonl> <seconds>
#import <AppKit/AppKit.h>
#include <sys/sysctl.h>

static FILE *output;

@interface SinkView : NSView
@end

@implementation SinkView
- (BOOL)acceptsFirstResponder { return YES; }
- (void)record:(NSEvent *)event kind:(const char *)kind {
  NSString *chars = @"";
  if (event.type == NSEventTypeKeyDown || event.type == NSEventTypeKeyUp) {
    chars = event.charactersIgnoringModifiers ?: @"";
  }
  fprintf(output, "{\"kind\":\"%s\",\"keycode\":%u,\"flags\":%lu,\"repeat\":%d,\"chars_len\":%lu}\n",
          kind, event.keyCode, (unsigned long)(event.modifierFlags & NSEventModifierFlagDeviceIndependentFlagsMask),
          (event.type == NSEventTypeKeyDown && event.isARepeat) ? 1 : 0, (unsigned long)chars.length);
  fflush(output);
}
- (void)keyDown:(NSEvent *)event {}
- (void)keyUp:(NSEvent *)event {}
- (void)flagsChanged:(NSEvent *)event {}
@end

int main(int argc, char **argv) {
  char model[128] = {0}; size_t size = sizeof(model);
  if (sysctlbyname("hw.model", model, &size, NULL, 0) || strncmp(model, "VirtualMac", 10)) return 70;
  if (argc < 3) return 64;
  output = fopen(argv[1], "w");
  double seconds = atof(argv[2]);
  @autoreleasepool {
    NSApplication *app = [NSApplication sharedApplication];
    [app setActivationPolicy:NSApplicationActivationPolicyRegular];
    // Cmd+V becomes paste: through the Edit menu's key equivalent, as in any app.
    NSMenu *menu = [[NSMenu alloc] init];
    NSMenuItem *editItem = [[NSMenuItem alloc] init];
    NSMenu *edit = [[NSMenu alloc] initWithTitle:@"Edit"];
    [edit addItemWithTitle:@"Paste" action:@selector(paste:) keyEquivalent:@"v"];
    editItem.submenu = edit;
    [menu addItem:[[NSMenuItem alloc] init]];
    [menu addItem:editItem];
    app.mainMenu = menu;
    NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(200, 200, 480, 240)
                                                   styleMask:NSWindowStyleMaskTitled
                                                     backing:NSBackingStoreBuffered
                                                       defer:NO];
    SinkView *view = [[SinkView alloc] initWithFrame:window.contentView.bounds];
    // Ordinary typing and pastes land in a text view, so delivered text can be compared.
    NSTextView *text = [[NSTextView alloc] initWithFrame:view.bounds];
    [view addSubview:text];
    window.contentView = view;
    window.title = @"azad-capture sink";
    [window makeKeyAndOrderFront:nil];
    [window makeFirstResponder:text];
    // Records every key event dispatched to this application, independent of responder state.
    [NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskKeyDown | NSEventMaskKeyUp | NSEventMaskFlagsChanged
                                          handler:^NSEvent *(NSEvent *event) {
      const char *kind = event.type == NSEventTypeKeyDown ? "down" : event.type == NSEventTypeKeyUp ? "up" : "flags";
      [view record:event kind:kind];
      return event;
    }];
    // Media and brightness keys arrive as system-defined events, which only a global monitor sees.
    [NSEvent addGlobalMonitorForEventsMatchingMask:NSEventMaskSystemDefined handler:^(NSEvent *event) {
      fprintf(output, "{\"kind\":\"system\",\"subtype\":%d,\"data1\":%ld}\n", (int)event.subtype, (long)event.data1);
      fflush(output);
    }];
    [app activateIgnoringOtherApps:YES];
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(0.5 * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
      [app activateIgnoringOtherApps:YES];
      [window makeKeyAndOrderFront:nil];
      fprintf(output, "{\"kind\":\"ready\",\"active\":%d,\"key_window\":%d}\n", app.isActive ? 1 : 0, window.isKeyWindow ? 1 : 0);
      fflush(output);
    });
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(seconds * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
      NSData *json = [NSJSONSerialization dataWithJSONObject:@{@"kind": @"text", @"value": text.string ?: @""} options:0 error:nil];
      fprintf(output, "%s\n", [[NSString alloc] initWithData:json encoding:NSUTF8StringEncoding].UTF8String);
      fprintf(output, "{\"kind\":\"done\",\"active\":%d,\"key_window\":%d}\n", app.isActive ? 1 : 0, window.isKeyWindow ? 1 : 0);
      fclose(output);
      exit(0);
    });
    [app run];
  }
}

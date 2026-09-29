// Competing HID event tap inserted at the head: counts key events and, with --consume, swallows
// them so nothing later in the chain (including the foreground app) sees them.
// Usage: tap <seconds> [--consume]
#import <ApplicationServices/ApplicationServices.h>
#include <string.h>
#include <sys/sysctl.h>

static bool consume;
static unsigned downs, ups, flags;

static CGEventRef callback(CGEventTapProxy proxy, CGEventType type, CGEventRef event, void *info) {
  if (type == kCGEventKeyDown) downs++;
  else if (type == kCGEventKeyUp) ups++;
  else if (type == kCGEventFlagsChanged) flags++;
  else return event;
  if (type != kCGEventFlagsChanged) {
    printf("{\"event\":\"tap_key\",\"type\":\"%s\",\"keycode\":%lld}\n", type == kCGEventKeyDown ? "down" : "up",
           CGEventGetIntegerValueField(event, kCGKeyboardEventKeycode));
  }
  return consume ? NULL : event;
}

int main(int argc, char **argv) {
  char model[128] = {0}; size_t size = sizeof(model);
  if (sysctlbyname("hw.model", model, &size, NULL, 0) || strncmp(model, "VirtualMac", 10)) return 70;
  setbuf(stdout, NULL);
  double seconds = argc > 1 ? atof(argv[1]) : 10;
  consume = argc > 2 && !strcmp(argv[2], "--consume");
  CGEventMask mask = CGEventMaskBit(kCGEventKeyDown) | CGEventMaskBit(kCGEventKeyUp) | CGEventMaskBit(kCGEventFlagsChanged);
  CFMachPortRef tap = CGEventTapCreate(kCGHIDEventTap, kCGHeadInsertEventTap,
                                       consume ? kCGEventTapOptionDefault : kCGEventTapOptionListenOnly,
                                       mask, callback, NULL);
  if (!tap) { printf("{\"event\":\"tap_failed\"}\n"); return 2; }
  CFRunLoopAddSource(CFRunLoopGetCurrent(), CFMachPortCreateRunLoopSource(NULL, tap, 0), kCFRunLoopDefaultMode);
  CGEventTapEnable(tap, true);
  printf("{\"event\":\"tap_ready\",\"consume\":%d}\n", consume);
  CFRunLoopRunInMode(kCFRunLoopDefaultMode, seconds, false);
  printf("{\"event\":\"tap_done\",\"downs\":%u,\"ups\":%u,\"flags\":%u}\n", downs, ups, flags);
}

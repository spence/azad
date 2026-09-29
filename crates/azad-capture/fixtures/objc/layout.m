// Selects a keyboard input source in the guest's GUI session, enabling it first if needed.
// Usage: layout <input-source-id>   e.g. com.apple.keylayout.French, com.apple.keylayout.US
#import <Carbon/Carbon.h>
#include <string.h>
#include <sys/sysctl.h>

int main(int argc, char **argv) {
  char model[128] = {0}; size_t size = sizeof(model);
  if (sysctlbyname("hw.model", model, &size, NULL, 0) || strncmp(model, "VirtualMac", 10)) return 70;
  if (argc < 2) return 64;
  CFStringRef wanted = CFStringCreateWithCString(NULL, argv[1], kCFStringEncodingUTF8);
  CFDictionaryRef filter = CFDictionaryCreate(NULL, (const void **)&kTISPropertyInputSourceID,
                                              (const void **)&wanted, 1, &kCFTypeDictionaryKeyCallBacks,
                                              &kCFTypeDictionaryValueCallBacks);
  CFArrayRef sources = TISCreateInputSourceList(filter, true);
  if (!sources || CFArrayGetCount(sources) == 0) { printf("{\"event\":\"layout_missing\"}\n"); return 2; }
  TISInputSourceRef source = (TISInputSourceRef)CFArrayGetValueAtIndex(sources, 0);
  OSStatus enabled = TISEnableInputSource(source);
  OSStatus selected = TISSelectInputSource(source);
  TISInputSourceRef current = TISCopyCurrentKeyboardLayoutInputSource();
  CFStringRef id = TISGetInputSourceProperty(current, kTISPropertyInputSourceID);
  char name[256] = {0};
  CFStringGetCString(id, name, sizeof(name), kCFStringEncodingUTF8);
  printf("{\"event\":\"layout\",\"enabled\":%d,\"selected\":%d,\"current\":\"%s\"}\n", (int)enabled, (int)selected, name);
  return strcmp(name, argv[1]) == 0 ? 0 : 3;
}

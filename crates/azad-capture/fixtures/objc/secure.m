// Holds Secure Input on for a number of seconds, as a password field would.
// Usage: secure <seconds>
#import <Carbon/Carbon.h>
#include <stdio.h>
#include <string.h>
#include <sys/sysctl.h>
#include <unistd.h>

int main(int argc, char **argv) {
  // Same guard as the other fixtures: a VirtualMac guest, or the owner-attended session.
  char model[128] = {0}; size_t size = sizeof(model);
#ifdef AZAD_OWNER_SESSION
  const char *owner = getenv("AZAD_OWNER_SESSION");
  if (!owner || strcmp(owner, "1")) return 70;
#else
  if (sysctlbyname("hw.model", model, &size, NULL, 0) || strncmp(model, "VirtualMac", 10)) return 70;
#endif
  int seconds = argc > 1 ? atoi(argv[1]) : 10;
  EnableSecureEventInput();
  printf("{\"event\":\"secure_on\",\"enabled\":%d}\n", IsSecureEventInputEnabled());
  fflush(stdout);
  sleep(seconds);
  DisableSecureEventInput();
  printf("{\"event\":\"secure_off\",\"enabled\":%d}\n", IsSecureEventInputEnabled());
}

#!/bin/bash
# Secure Input check for the tap build: Option+Space hold with Secure Input off, on, then off.
set -u
rm -rf /tmp/azt /tmp/siy && mkdir -p /tmp/azt && tar xzf /tmp/siy-stage.tgz -C /tmp/azt && mv /tmp/azt/siy /tmp/siy
codesign --force --sign - /tmp/azt/bin/Sink.app 2>/dev/null
B=/tmp/azt/bin; D=/tmp/siy/run; mkdir -p $D
LOG=~/Library/Logs/Azad/input.log
codesign -dvvv /Applications/Azad.app 2>&1 | grep -E "^(Identifier|CDHash|TeamIdentifier)=" > $D/guest-identity.txt
for phase in before secure after; do
  script=/tmp/siy/hold.json; [ $phase = secure ] && script=/tmp/siy/secure-hold.json
  wc -l < $LOG | tr -d ' ' > $D/$phase-applog-start
  open -n $B/Sink.app --args $D/$phase-sink.jsonl 10
  sleep 3
  ( for i in $(seq 1 12); do ioreg -l -w 0 | grep -o 'kCGSSessionSecureInputPID"=[0-9]*' | head -1; sleep 0.5; done ) > $D/$phase-secure-holder.txt &
  echo admin | sudo -S -p '' $B/fixture-keyboard --script $script --settle-ms 1500 > $D/$phase-source.jsonl 2>&1
  wait
  sleep 6
  tail -n +$(( $(cat $D/$phase-applog-start) + 1 )) $LOG > $D/$phase-app.jsonl
done
ls $D

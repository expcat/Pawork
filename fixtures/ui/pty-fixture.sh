#!/bin/sh
# Pawork UI fixture PTY：确定性输出与回显，无日期/随机/网络。
printf 'fixture pty ready\n'
printf 'profile: ui-fixture\n'
while IFS= read -r line; do
  case "$line" in
    exit)
      printf 'fixture pty closed\n'
      exit 0
      ;;
    resize)
      printf 'fixture pty resized\n'
      ;;
    *)
      printf 'echo: %s\n' "$line"
      ;;
  esac
done

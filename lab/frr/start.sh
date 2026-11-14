#!/bin/sh
# Sets root's password from NETTERM_LAB_PASSWORD, starts sshd with host keys
# of this container's own, then FRR as the image normally does.
set -e
if [ -z "$NETTERM_LAB_PASSWORD" ]; then
  echo "netterm-frr: NETTERM_LAB_PASSWORD is not set" >&2
  exit 1
fi
echo "root:$NETTERM_LAB_PASSWORD" | chpasswd
ssh-keygen -A
/usr/sbin/sshd
exec /usr/lib/frr/docker-start

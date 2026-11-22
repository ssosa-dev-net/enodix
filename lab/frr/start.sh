#!/bin/sh
# Sets the passwords of root and vtysh from ENODIX_LAB_PASSWORD, starts
# sshd with host keys of this container's own, then FRR as the image
# normally does.
set -e
if [ -z "$ENODIX_LAB_PASSWORD" ]; then
  echo "enodix-frr: ENODIX_LAB_PASSWORD is not set" >&2
  exit 1
fi
echo "root:$ENODIX_LAB_PASSWORD" | chpasswd
echo "vtysh:$ENODIX_LAB_PASSWORD" | chpasswd
ssh-keygen -A
/usr/sbin/sshd
exec /usr/lib/frr/docker-start

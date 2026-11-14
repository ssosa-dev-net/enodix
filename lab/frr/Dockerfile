# FRR with an SSH server, for the containerlab device suite: the FRR image
# has no SSH of its own. Root logs in with the password the container gets
# in NETTERM_LAB_PASSWORD (from the topology) and gets a shell, where
# `vtysh -c '...'` runs FRR commands.
#
#   docker build -t netterm-frr:latest lab/frr
#
FROM quay.io/frrouting/frr:10.2.1

RUN apk add --no-cache openssh-server \
 && sed -i -e 's/^#\?PermitRootLogin.*/PermitRootLogin yes/' \
           -e 's/^#\?PasswordAuthentication.*/PasswordAuthentication yes/' \
           /etc/ssh/sshd_config

COPY start.sh /usr/local/bin/netterm-start
RUN chmod +x /usr/local/bin/netterm-start

ENTRYPOINT ["/sbin/tini", "--", "/usr/local/bin/netterm-start"]

#!/bin/sh
# The one executable behind every script fixture. Tests link to this file and
# write their body beside the link, so the inode that is executed is never one
# a test opened for writing.
exec /bin/sh "${0}.body" "$@"

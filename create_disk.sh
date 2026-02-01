#!/bin/bash

# create a raw disk image of 1G, format it with btrfs filesystem.
# mount it to PWD/target/disk and set permissions to 777.
# If the disk image or mount directory already exists, do nothing.
# This script requires sudo privileges to run.


#!/bin/bash -eux

sudo setcap 'cap_net_bind_service=+ep' $HOME/.cargo/bin/hypershare

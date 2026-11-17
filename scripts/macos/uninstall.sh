#!/bin/bash
set -euo pipefail
"$(dirname "$0")/login-start.sh" disable
"$(dirname "$0")/stop.sh"
printf '%s\n' 'LANLink Agent stopped and login startup disabled. Move /Applications/LANLink.app to Trash to remove the app.' 'Device identity, paired devices, messages and received files are retained in the Agent data directory.'

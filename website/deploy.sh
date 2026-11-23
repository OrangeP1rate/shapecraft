#!/bin/sh
# Copies the site to the web server over ssh: index.html, web.config, media/ and press/.
#   sh website/deploy.sh
#   WEB_HOST=… WEB_DIR=… sh website/deploy.sh   (or put both in website/deploy.env)
# The server side is a non-admin "deploy" account that can write only the site folder (see README.md).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
# The server's name and folder live in deploy.env beside this script (not in git): WEB_HOST=deploy@… and WEB_DIR=C:/….
[ -f "$here/deploy.env" ] && . "$here/deploy.env"
host=${WEB_HOST:?set WEB_HOST (deploy@server) in website/deploy.env}
dir=${WEB_DIR:-C:/inetpub/wwwroot/jackharvest.com/shapecraft}
key=${WEB_KEY:-$HOME/.ssh/id_ed25519_web}
# The press kit's .zip is put together here (not in git): its pictures, the video and the logo.
(cd "$here" && rm -f press/ShapeCraft-press-kit.zip && zip -jq press/ShapeCraft-press-kit.zip media/print.jpg media/thread.jpg media/thread.mp4 press/handheld.jpg media/handheld.mp4 media/gr-before.jpg media/gr-after.jpg media/home-get-started.jpg media/workspace.jpg media/icon.svg media/icon-256.png media/og-card.jpg press/shapecraft-launch.mp4)
# Media first, the page last, so a visitor mid-deploy never gets a page pointing at missing files.
scp -i "$key" -o IdentitiesOnly=yes -o BatchMode=yes -r "$here/media" "$here/press" "$host:/$dir/"
scp -i "$key" -o IdentitiesOnly=yes -o BatchMode=yes "$here/web.config" "$here/index.html" "$host:/$dir/"
echo "deployed to $host:$dir"

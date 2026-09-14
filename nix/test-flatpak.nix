# a flatpak runtime and app of our own for the boot test, as two bundles, so the test needs nothing
# from flathub. the runtime is a static busybox and a static gdbus, the app one shell script
{ pkgs }:
let
  inherit (pkgs) lib;
  runtime = "dev.eclipse.TestPlatform";
  app = "dev.eclipse.TestApp";
  branch = "test";
  # what the app does: reads the file the document portal gave it, then the same file where it is in
  # home, asks the desktop portal whether there is a network, and asks the user manager for
  # something, which the bus proxy flatpak puts in front of the session bus does not pass on
  probe = pkgs.writeText "probe" ''
    #!/bin/sh
    document=$1
    direct=$2
    echo "cgroup: $(cat /proc/self/cgroup)"
    cat "$document" && echo document-read
    cat "$direct" && echo direct-read
    gdbus call --session --timeout 10 --dest org.freedesktop.portal.Desktop \
      --object-path /org/freedesktop/portal/desktop \
      --method org.freedesktop.portal.NetworkMonitor.GetAvailable && echo portal-answered
    gdbus call --session --timeout 10 --dest org.freedesktop.systemd1 \
      --object-path /org/freedesktop/systemd1 \
      --method org.freedesktop.DBus.Peer.Ping && echo manager-answered
    echo finished
  '';
in
pkgs.runCommand "eclipse-test-flatpak" { nativeBuildInputs = [ pkgs.flatpak ]; } ''
  export HOME=$TMPDIR

  # a runtime's files are its usr, and build-export wants the files folder a build-init makes as well
  mkdir -p platform/usr/bin platform/files
  cp ${pkgs.pkgsStatic.busybox}/bin/busybox platform/usr/bin/busybox
  for tool in $(platform/usr/bin/busybox --list); do
    [ -e platform/usr/bin/$tool ] || ln -s busybox platform/usr/bin/$tool
  done
  cp ${lib.getBin pkgs.pkgsStatic.glib}/bin/gdbus platform/usr/bin/gdbus
  cat > platform/metadata <<EOF
  [Runtime]
  name=${runtime}
  runtime=${runtime}/x86_64/${branch}
  sdk=${runtime}/x86_64/${branch}
  EOF
  flatpak build-export --runtime --disable-fsync repo platform ${branch}

  mkdir -p testapp/files/bin testapp/export
  install -m 755 ${probe} testapp/files/bin/probe
  cat > testapp/metadata <<EOF
  [Application]
  name=${app}
  runtime=${runtime}/x86_64/${branch}
  sdk=${runtime}/x86_64/${branch}
  command=probe

  [Context]
  shared=network;
  EOF
  flatpak build-export --disable-fsync repo testapp ${branch}

  mkdir -p $out
  flatpak build-bundle --runtime repo $out/platform.flatpak ${runtime} ${branch}
  flatpak build-bundle repo $out/app.flatpak ${app} ${branch}
''

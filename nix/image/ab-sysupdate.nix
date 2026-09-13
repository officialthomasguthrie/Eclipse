# the running side of the a/b slots. default.nix lays out slot a, eclipse-flash adds slot b.
#
# systemd-boot takes a try off a uki's counter each time it starts it, and systemd-bless-boot drops
# the counter once boot-complete.target is reached. a uki with no tries left sorts behind the others,
# so three failed boots of a new version start the old one again. systemd-sysupdate writes a new
# version into the partitions labelled _empty or holding the older version, never the running one
{
  config,
  lib,
  ...
}:
let
  inherit (config.system.image) id;

  # updates come from a directory on persist until there is a channel to download them from.
  # sysupdate still checks every file against SHA256SUMS and its signature
  source = "file:///var/lib/eclipse/updates/";

  partition = type: label: file: {
    Transfer.ProtectVersion = "%A";
    Source = {
      Type = "url-file";
      Path = source;
      # the partition uuid is in the file name. the initrd finds the store by uuids made from the
      # usrhash in the uki, so a written partition has to get the uuid its uki expects
      MatchPattern = [
        "${id}_@v_@u.${file}.zst"
        "${id}_@v_@u.${file}"
      ];
    };
    Target = {
      Type = "partition";
      Path = "auto";
      MatchPattern = "${label}_@v";
      MatchPartitionType = type;
      ReadOnly = "yes";
      InstancesMax = 2;
    };
  };

  # a boot is good once syzygy has written the host profile and greetd has started the session
  checks =
    lib.optional config.eclipse.syzygy.enable "syzygy.service"
    ++ lib.optional config.services.greetd.enable "greetd.service";
in
{
  systemd.sysupdate = {
    enable = true;
    # written in file name order, so the uki that makes a version bootable comes after its store
    transfers = {
      "10-store-verity" = partition "usr-x86-64-verity" "store-verity" "verity";
      "20-store" = partition "usr-x86-64" "store" "store";
      "30-uki" = {
        Transfer.ProtectVersion = "%A";
        Source = {
          Type = "url-file";
          Path = source;
          MatchPattern = "${id}_@v.efi";
        };
        Target = {
          Type = "regular-file";
          Path = "/EFI/Linux";
          PathRelativeTo = "boot";
          MatchPattern = [
            "${id}_@v+@l-@d.efi"
            "${id}_@v+@l.efi"
            "${id}_@v.efi"
          ];
          # not 0444: on vfat that sets the read-only attribute, and systemd-boot does not count the
          # boots of a read-only file
          Mode = "0644";
          TriesLeft = 3;
          TriesDone = 0;
          InstancesMax = 2;
        };
      };
    };
  };
  # an update runs when the owner asks for one, not on a timer
  systemd.timers.systemd-sysupdate.wantedBy = lib.mkForce [ ];

  # the esp of the drive this system booted from. udev names that drive's partitions under
  # by-designator, so the esp of a host disk is never the one mounted here. it is mounted when
  # something reads it and let go two minutes later, so a pulled drive seldom leaves it dirty
  fileSystems."/boot" = {
    device = "/dev/disk/by-designator/esp";
    fsType = "vfat";
    options = [
      "umask=0077"
      "nosuid"
      "nodev"
      "noexec"
      "nofail"
      "x-systemd.automount"
      "x-systemd.idle-timeout=2min"
    ];
  };

  # when one of the checks fails to start, the target is never reached and the counter keeps running
  systemd.targets.boot-complete = {
    requires = checks;
    after = checks ++ [ "graphical.target" ];
  };
}

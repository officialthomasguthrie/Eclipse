# a/b slots updated by systemd-sysupdate from a static https dir. not imported yet.
#
# the esp holds eclipse_<version>.efi ukis under /EFI/Linux, systemd-boot picks the newest one whose
# boot counter isn't used up. two store and two store-verity partitions are the slots. sysupdate pulls a
# new uki + store + verity into the inactive slot; next boot gets three tries before falling back.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  updateUrl = "https://updates.eclipse.invalid/stable/"; # per channel
in
{
  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = false; # never write the host's firmware variables

  systemd.sysupdate = {
    enable = true;
    transfers = {
      "10-uki" = {
        Source = {
          Type = "url-file";
          Path = updateUrl;
          MatchPattern = "eclipse_@v.efi";
        };
        Target = {
          Type = "regular-file";
          Path = "/EFI/Linux";
          PathRelativeTo = "esp";
          MatchPattern = [
            "eclipse_@v+@l-@d.efi"
            "eclipse_@v+@l.efi"
            "eclipse_@v.efi"
          ];
          Mode = "0444";
          TriesLeft = 3;
          TriesDone = 0;
          InstancesMax = 2;
        };
      };
      "20-store-verity" = {
        Source = {
          Type = "url-file";
          Path = updateUrl;
          MatchPattern = "eclipse_@v.verity";
        };
        Target = {
          Type = "partition";
          Path = "auto";
          MatchPattern = "store-verity_@v";
          MatchPartitionType = "usr-verity";
          ReadOnly = "yes";
        };
      };
      "30-store" = {
        Source = {
          Type = "url-file";
          Path = updateUrl;
          MatchPattern = "eclipse_@v.store";
        };
        Target = {
          Type = "partition";
          Path = "auto";
          MatchPattern = "store_@v";
          MatchPartitionType = "usr";
          ReadOnly = "yes";
        };
      };
    };
  };

  # the session has to reach boot-complete.target before a slot counts as good
  systemd.additionalUpstreamSystemUnits = [
    "boot-complete.target"
    "systemd-bless-boot.service"
  ];
}

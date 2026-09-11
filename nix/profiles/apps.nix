# the apps every image has next to the shell: firefox, zed and podman here, helix, zellij and fish
# in base.nix, ghostty with umbra. none of them reports home or asks for an account
{ pkgs, ... }:
let
  # zed only reads settings from the owner's home, so they are written there once, when there is
  # no file yet. the owner can change them afterwards
  zedSettings = builtins.toJSON {
    telemetry = {
      diagnostics = false;
      metrics = false;
    };
    auto_update = false;
  };
in
{
  programs.firefox = {
    enable = true;
    # nothing goes to mozilla, no first run pages, no account, and the new tab page is a search
    # field and nothing else
    policies = {
      DisableTelemetry = true;
      DisableFirefoxStudies = true;
      DisablePocket = true;
      DisableFirefoxAccounts = true;
      DisableFeedbackCommands = true;
      DontCheckDefaultBrowser = true;
      NoDefaultBookmarks = true;
      OverrideFirstRunPage = "";
      OverridePostUpdatePage = "";
      SkipTermsOfUse = true;
      UserMessaging = {
        WhatsNew = false;
        ExtensionRecommendations = false;
        FeatureRecommendations = false;
        UrlbarInterventions = false;
        SkipOnboarding = true;
        MoreFromMozilla = false;
        FirefoxLabs = false;
        Locked = true;
      };
      FirefoxHome = {
        Search = true;
        Weather = false;
        TopSites = false;
        SponsoredTopSites = false;
        Highlights = false;
        Pocket = false;
        Stories = false;
        SponsoredPocket = false;
        SponsoredStories = false;
        Snippets = false;
        Locked = true;
      };
      FirefoxSuggest = {
        WebSuggestions = false;
        SponsoredSuggestions = false;
        ImproveSuggest = false;
        OnlineEnabled = false;
        Locked = true;
      };
      # the chatbot sidebar and the rest send what is on the page to cloud models
      GenerativeAI = {
        Enabled = false;
        Chatbot = false;
        LinkPreviews = false;
        SmartWindow = false;
        TabGroups = false;
        Locked = true;
      };
    };
    preferences = {
      # the privacy notice tab on the first start
      "datareporting.policy.dataSubmissionPolicyBypassNotification" = true;
      # the recommendations in the add-ons manager load from mozilla
      "extensions.getAddons.showPane" = false;
      # safe browsing keeps its block lists but does not send downloads to google for a verdict
      "browser.safebrowsing.downloads.remote.enabled" = false;
      # both ask detectportal.firefox.com every so often
      "network.captive-portal-service.enabled" = false;
      "network.connectivity-service.enabled" = false;
    };
  };

  environment.systemPackages = [ pkgs.zed-editor ];
  systemd.user.tmpfiles.rules = [
    "d %h/.config/zed 0755 - - -"
    "f %h/.config/zed/settings.json 0644 - - - ${zedSettings}"
  ];

  virtualisation.podman = {
    enable = true;
    # docker in scripts runs podman. there is no docker daemon
    dockerCompat = true;
  };
  # rootless containers map their users into the owner's ranges
  users.users.eclipse = {
    subUidRanges = [
      {
        startUid = 100000;
        count = 65536;
      }
    ];
    subGidRanges = [
      {
        startGid = 100000;
        count = 65536;
      }
    ];
  };
}

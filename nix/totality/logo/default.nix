# the logo in characters: a black hole, its disc and photon ring in warm colours. eclipse-logo.txt is
# the full size and eclipse-logo-small.txt half of it for small terminals. both have fastfetch's $1
# to $9 in front of each change of colour, and every line starts with its colour. these are the
# colours, from xterm's 256, and the forms of each logo the image installs
{ lib }:
let
  colours = [
    "38;5;52"
    "38;5;94"
    "38;5;130"
    "38;5;166"
    "38;5;208"
    "38;5;214"
    "38;5;220"
    "38;5;229"
    "38;5;231"
  ];
  marks = lib.genList (i: "$" + toString (i + 1)) (builtins.length colours);
  # the escape character, which a nix string cannot hold, from a json one
  esc = builtins.fromJSON (
    lib.concatStrings [
      "\""
      "\\"
      "u001b\""
    ]
  );
  forms =
    file:
    let
      lines = lib.splitString "\n" (lib.removeSuffix "\n" (builtins.readFile file));
      eachLine = f: lib.concatMapStrings (line: f line + "\n") lines;
    in
    {
      inherit file;
      rows = builtins.length lines;
      columns = lib.foldl' lib.max 0 (
        map (line: lib.stringLength (lib.replaceStrings marks (map (_: "") colours) line)) lines
      );
      # without colours, what fastfetch --pipe prints
      plain = eachLine (lib.replaceStrings marks (map (_: "") colours));
      # for a terminal. each line ends in a reset, so a line on its own keeps its colours
      ansi = eachLine (
        line: lib.replaceStrings marks (map (c: "${esc}[${c}m") colours) line + "${esc}[0m"
      );
      # for agetty, which reads a backslash as the start of an escape: \e is the escape character
      issue = eachLine (
        line:
        lib.replaceStrings marks (map (c: "\\e[${c}m") colours) (
          lib.replaceStrings [ "\\" ] [ "\\\\" ] line
        )
        + "\\e{reset}"
      );
    };
in
forms ./eclipse-logo.txt
// {
  inherit colours;
  small = forms ./eclipse-logo-small.txt;
}

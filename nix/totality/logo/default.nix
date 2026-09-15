# the logo in characters: a black hole, its disc and photon ring in shades of one warm gold.
# eclipse-logo.txt is the full size and eclipse-logo-small.txt half of it for small terminals. both
# have fastfetch's $1 to $9 in front of each change of shade, and every line starts with its shade.
# these are the nine shades from dim to pale, in true colour, and the forms of each logo the image
# installs
{ lib }:
let
  colours = [
    "38;2;115;84;38"
    "38;2;142;104;46"
    "38;2;171;124;54"
    "38;2;196;143;64"
    "38;2;206;159;90"
    "38;2;215;176;117"
    "38;2;224;192;144"
    "38;2;232;208;171"
    "38;2;240;223;199"
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
  # the full size logo drawn in type half the terminal's size, as a picture for terminals that show
  # images. it fills the cells of the small logo
  picture = {
    file = ./eclipse-logo.png;
    columns = 59;
    rows = 16;
  };
}

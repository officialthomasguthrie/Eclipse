# the logo in characters: a black hole in ice, its disc, its photon ring and the haze around them.
# rift-logo.txt has the characters and rift-logo.colours the colour of each of them as r;g;b, with -
# for a space. these are the forms of the logo the image installs
{ lib }:
let
  # the escape character, which a nix string cannot hold, from a json one
  esc = builtins.fromJSON (
    lib.concatStrings [
      "\""
      "\\"
      "u001b\""
    ]
  );
  read = file: lib.splitString "\n" (lib.removeSuffix "\n" (builtins.readFile file));
  texts = read ./rift-logo.txt;
  # each line as its cells, a character and its colour, null for a space
  lines = lib.zipListsWith (
    text: colours:
    lib.zipListsWith (char: colour: {
      inherit char;
      colour = if colour == "-" then null else colour;
    }) (lib.stringToCharacters text) (if colours == "" then [ ] else lib.splitString " " colours)
  ) texts (read ./rift-logo.colours);
  eachLine = f: lib.concatMapStrings (line: f line + "\n") lines;
  brightest = colour: lib.foldl' lib.max 0 (map lib.toInt (lib.splitString ";" colour));
in
{
  rows = builtins.length lines;
  columns = lib.foldl' lib.max 0 (map lib.stringLength texts);
  # without colours, for what is not a terminal
  plain = eachLine (lib.concatMapStrings (cell: cell.char));
  # every character in its own colour. each line ends in a reset, so a line on its own keeps its
  # colours
  ansi = eachLine (
    line:
    lib.concatMapStrings (
      cell: if cell.colour == null then cell.char else "${esc}[38;2;${cell.colour}m${cell.char}"
    ) line
    + "${esc}[0m"
  );
  # for agetty, which reads a backslash as the start of an escape: \e is the escape character. the
  # kernel's console makes one of its sixteen colours of a true colour from the parts over half the
  # brightest, so the faint blues of the haze would come out in its full blue. they are black there,
  # as on the page the logo was drawn on
  issue = eachLine (
    line:
    lib.concatMapStrings (
      cell:
      let
        char = lib.replaceStrings [ "\\" ] [ "\\\\" ] cell.char;
      in
      if cell.colour == null then
        char
      else if brightest cell.colour < 70 then
        "\\e[30m${char}"
      else
        "\\e[38;2;${cell.colour}m${char}"
    ) line
    + "\\e{reset}"
  );
}

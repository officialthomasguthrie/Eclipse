# phase 0: it boots. console, shell, splash, aura's backend if a model is there. no compositor yet.
{ ... }:
{
  boot.kernelParams = [
    "console=ttyS0,115200"
    "console=tty1"
  ];
  services.getty.autologinUser = "eclipse";

  eclipse.totality.enable = true;
  eclipse.syzygy.enable = true;
  eclipse.aura.enable = true;
  eclipse.umbra.enable = false;
  eclipse.corona.enable = false;
  eclipse.penumbra.enable = false;
  eclipse.vault.enable = true;

  environment.etc."eclipse/phase".text = "0\n";
}

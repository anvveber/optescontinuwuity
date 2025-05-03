{ inputs, ... }:
{
  testSystem = inputs.nixpkgs.lib.nixosSystem {
    system = "x86_64-linux";
    modules = [
      {
        fileSystems."/".device = "/dev/sda1";
        boot.loader.grub.enable = true; # Enable GRUB bootloader
        boot.loader.grub.device = "/dev/sda"; # Change to your boot device
      }
      inputs.self.nixosModules.continuwuity
      {
        config = {
          services.continuwuity = {
            enable = true;
            port = 10000;
            address = "::1";
            domain = "example.com";
            settings = {
              registration_token_file = ./test.nix;
            };
          };
        };
      }
    ];
  };
}

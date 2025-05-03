# These are the configuration options available in the continuwuity.toml file in a nixified way
{ config, lib }:
let
  cfg = config.services.continuwuity;
in
{
  options =
    let
      mkDisableOption =
        name:
        lib.mkOption {
          description = "Whether or not to enable ${name}";
          type = lib.types.bool;
          default = true;
          example = false;
        };
    in
    {

      database_backup_path = lib.mkOption {
        description = ''
          continuwuity supports online database backups using RocksDB's
          Backup engine API. To use this, set a database backup path that
          conduwuit can write to.
        '';
        type = lib.types.str;
        default = "${cfg.dataDir}/continuwuity-db-backups";
        example = "/var/lib/continuwuity-db-backups";
      };

      database_backups_to_keep = lib.mkOption {
        description = ''
          The amount of online RocksDB database backups to keep/retain,
          if using "database_backup_path", before deleting the oldest one.
        '';
        type = lib.types.int;
        default = 1;
        example = 10;
      };

      new_user_displayname_suffix = lib.mkOption {
        description = ''
          Text which will be added to the end of the user's displayname upon
          registration with a space before the text. In Conduit, this was the
          lightning bolt emoji.
        '';
        type = lib.types.str;
        default = "🏳️‍⚧️";
        example = "🏳️‍⚧️";
      };

      allow_announcements_check = mkDisableOption "regular GET request for announcements";

      registration_token_file = lib.mkOption {
        description = ''
          The file the registration token is stored in.
        '';
        type = lib.types.path;
        readOnly = true;
      };

      allow_registration = lib.mkEnableOption "registration of new users";
      allow_encryption = mkDisableOption "creation of encrypted rooms";
      allow_federation = mkDisableOption "federation with other servers";

      trusted_servers = lib.mkOption {
        description = "Servers trusted with signing server keys.";
        type = lib.types.listOf lib.types.str;
        default = [ "matrix.org" ];
      };

      extraConfig = lib.mkOption {
        description = "Everything in the config scheme which isn't nixified yet. Note that you are on your own here.";
        type = lib.types.attrs;
        default = { };
        example = {
          max_request_size = 2000000;
        };
      };
    };
}

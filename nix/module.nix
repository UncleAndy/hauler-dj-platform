# Real-time scheduling for the DJ audio thread (ADR-0010).
#
# The priority is granted by the system, never requested by the application: the app measures the
# policy it actually received and warns when it is not real-time. Members of the `audio` group get
# an rtprio allowance, and a launcher runs the engine under `chrt -f <priority>`; threads created
# later — including the audio callback inside cpal, which is not our code — inherit the policy.
{ config, lib, ... }:
{
  options.services.dj-audio-rt = {
    enable = lib.mkEnableOption "real-time scheduling for the DJ audio thread";

    realtimePriority = lib.mkOption {
      type = lib.types.ints.between 1 95;
      default = 20;
      description = ''
        Real-time priority granted to members of the `audio` group. Values above 50 are pointless
        and unkind to system tasks; 20–30 is the range the measurements were taken in.
      '';
    };

    memlockUnlimited = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Allow locking audio memory, which every low-latency path wants.";
    };
  };

  config = lib.mkIf config.services.dj-audio-rt.enable {
    # Components that take priority through a broker (PipeWire) keep working.
    security.rtkit.enable = lib.mkDefault true;

    security.pam.loginLimits =
      [
        {
          domain = "@audio";
          item = "rtprio";
          value = toString config.services.dj-audio-rt.realtimePriority;
          type = "-";
        }
      ]
      ++ lib.optional config.services.dj-audio-rt.memlockUnlimited {
        domain = "@audio";
        item = "memlock";
        value = "unlimited";
        type = "-";
      };
  };
}

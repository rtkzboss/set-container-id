# set_container_id
[Download](https://github.com/rtkzboss/tools/releases/latest/download/set_container_id.exe)
```
Set the container ID of an Unreal Engine 4.27 IoStore container

Usage: set_container_id.exe [OPTIONS] <CONTAINER> [ID]

Arguments:
  <CONTAINER>
          Path to the container file

  [ID]
          Container ID override

          Defaults to the container file name, excluding the extension, platform ID, and patch designator. This is the same value Unreal uses

Options:
      --force
          Rewrite the container even if its ID is already the requested one

      --compression <COMPRESSION>
          Set the compression level of any modified compression blocks

          [default: default]
          [possible values: fastest, default, best]

      --dry-run
          Leave the container untouched and print out what would be done

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
```

## Oodle
Does not currently support oodle compression. If you see `todo: oodle compression`, set the `Pak File Compression Format` to `Zlib` in Unreal's advanced packaging settings, and the tool should work (at the expense of a slightly larger pak file). 

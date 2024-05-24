# set_container_id

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

  -h, --help
          Print help (see a summary with '-h')
```

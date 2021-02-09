# HyperShare

Kind of like OnionShare but for normal people: an interactive HTTP server that can both serve and accept files.

It has been designed to be used with basically any web browser. Users don't need special software to upload or download files.

## Why?

So you want to send a file to someone, but you can't or don't want to put the file on a cloud storage system. You could use nginx, SimpleHTTPServer, or some other HTTP, SSH, or FTP server, but most of your choices are either too large, too complicated, or don't implement enough of their respective protocol to be useful. Also, basically NONE of them gives you the option to inspect connections as they are served.

That is why I wrote this. HyperShare is a reasonably fast, single-threaded HTTP server designed for low-volume, high-bandwidth activity. Its key feature is that connections and download progress can be observed.

## Usage and Controls

HyperShare has three controls:
* Pressing Q will close the server and kill the interface.
* Pressing Space will toggle the server's enabled/disabled state. When disabling the server, all in-flight responses will be completed, but new requests will receive an error page instead of the requested resource.
* Pressing K will kill all current connections immediately, but new connections will still be accepted.

HyperShare supports various modes of operation. See `hypershare --help` for more information.

### Defaults

HyperShare will listen on `localhost:80` and serve your current working directory by default.

### Uploading

If enabled with `-u`, HyperShare will accept file uploads via POST requests. The appropriate HTML form is generated in directory listings. Files must be uploaded as `multipart/form-data`.

## Code Formatting
Use a nightly Rust toolchain to use the required `rustfmt` features.

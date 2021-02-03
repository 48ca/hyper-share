# Simple HTTP server with TUI

So you want to send a file to someone, but you can't or don't want to put the file on a cloud storage system. You could use nginx, SimpleHTTPServer, or some other HTTP or SSH or FTP or anything server, but most of your choices are either too large, too complicated, or don't implement enough of the HTTP protocol to be useful. Also, NONE of them give you the option to inspect connections as they are served.

That is why I wrote this. HTTP-TUI is a reasonably fast, single-threaded HTTP server designed for low-volume, high-bandwidth activity. It's key feature is that connections and download progress can be observed.

HTTP-TUI has two controls:
* Pressing Q will close the server and kill the interface.
* Pressing Space will toggle the server's enabled/disabled state. When disabling the server, all in-flight responses will be completed, but new requests will receive an error page instead of the requested resource.

Uploading capabilities will come soon.

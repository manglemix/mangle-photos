# mangle-photos
A personal app I created to quickly share photos across the local network

## Motivation
When I go on a trip with my friends, I'm usually the photographer. After a photo session, I need a way to efficiently share my photos with them.
However, the photos I take are often several megabytes, so sharing them over google drive can make the process very slow.
Instead, I created this Rust App that caches all images into memory, creates small WebP previews, zips all images for one convenient download, and deploys all these to a
http server that can be connected to locally (ie. to everyone connected to the same wifi). Now, people who visit the website will see a convenient preview for all images,
along with a Download All button to download every image easily.

## Execution
Reading and encoding of images is multithreaded with `Rayon`, with all served resources being leaked intentionally. This makes the http server code (done with `axum`)
much simpler, as references to these resources can be copied across all server threads and be read simultaneously without any synchronization primitives.

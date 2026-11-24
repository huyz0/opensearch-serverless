# The emulator's S3 front end takes `aws-chunked` bodies

## Serves

R15.5. The Java shell's AWS SDK sends a PUT over plain HTTP as `aws-chunked` (`Content-Encoding: aws-chunked`, the length of the object in `x-amz-decoded-content-length`, the body in signed or checksummed chunks). The front end stored the framing as the object, so a Java node could not run against the in-process emulator and the live mixed-fleet tests needed a RustFS binary. This decodes the framing, so a live Java node runs on the emulator.

## Change

`oss_blob::emulator::http::S3Server` reads a PUT whose `Content-Encoding` names `aws-chunked` chunk by chunk: `<hex size>[;chunk-signature=…]\r\n`, the data, `\r\n`, ending at a zero-size chunk and its trailers. The object is the decoded bytes and its length is `x-amz-decoded-content-length`. A body that does not parse is refused and nothing is stored. Chunk signatures are not verified (the request's own signature still is). Test support only.

## Acceptance criteria

- AC-1: a PUT whose chunks carry signatures stores the decoded bytes; so does one that ends in trailers, and one whose only chunk is the empty one.
- AC-2: a body that ends early, has a chunk size that is not hex, or lacks a chunk's line end is not stored.

## S3 request budget

None: test support, no store request is added.

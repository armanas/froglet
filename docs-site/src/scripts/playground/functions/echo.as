// The simplest function: it answers with its request.
// A response has to be JSON text, and a request is JSON text, so {"hello": "world"} answers {"hello":"world"}.
// Change respond to answer something else, then publish it.

function respond(request: string): string {
  return request;
}

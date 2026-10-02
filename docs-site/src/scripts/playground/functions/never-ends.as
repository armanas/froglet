// A function that never returns. Publish it and call it: Bob stops it at his time limit, and signs a receipt that says
// the function failed. Delete the loop, publish again, and it answers.

function respond(request: string): string {
  while (true) {} // remove this line and the function answers
  return '{"finished":true}';
}

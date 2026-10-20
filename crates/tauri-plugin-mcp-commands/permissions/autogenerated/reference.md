## Default Permission

Lets the webview start, stop and inspect the MCP server.

Reading connection configs (which contain the bearer token), rotating the
token and answering access requests are separate permissions:
`allow-connection-configs`, `allow-rotate-token`, `allow-respond-access`.

#### This default permission set includes the following:

- `allow-start`
- `allow-stop`
- `allow-status`

## Permission Table

<table>
<tr>
<th>Identifier</th>
<th>Description</th>
</tr>


<tr>
<td>

`mcp-commands:allow-connection-configs`

</td>
<td>

Enables the connection_configs command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`mcp-commands:deny-connection-configs`

</td>
<td>

Denies the connection_configs command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`mcp-commands:allow-respond-access`

</td>
<td>

Enables the respond_access command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`mcp-commands:deny-respond-access`

</td>
<td>

Denies the respond_access command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`mcp-commands:allow-rotate-token`

</td>
<td>

Enables the rotate_token command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`mcp-commands:deny-rotate-token`

</td>
<td>

Denies the rotate_token command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`mcp-commands:allow-start`

</td>
<td>

Enables the start command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`mcp-commands:deny-start`

</td>
<td>

Denies the start command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`mcp-commands:allow-status`

</td>
<td>

Enables the status command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`mcp-commands:deny-status`

</td>
<td>

Denies the status command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`mcp-commands:allow-stop`

</td>
<td>

Enables the stop command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`mcp-commands:deny-stop`

</td>
<td>

Denies the stop command without any pre-configured scope.

</td>
</tr>
</table>

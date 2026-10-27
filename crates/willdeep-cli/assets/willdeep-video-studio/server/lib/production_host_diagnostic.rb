# frozen_string_literal: true

require "digest"
require "json"

# Classify host failures without persisting provider bodies, URLs or credentials.
# Diagnostic labels never settle usage or grant permission to retry.
module ProductionHostDiagnostic
  module_function

  def describe(error)
    message = error.message.to_s
    status = message[/\bprovider returned HTTP ([45][0-9]{2})\b/, 1]&.to_i
    result = { "hostCode" => error.code, "category" => category(message, status), "diagnosticSHA256" => Digest::SHA256.hexdigest(message) }
    result["httpStatus"] = status if status
    JSON.generate(result)
  end

  def category(message, status)
    return "upstream_http_error" if status
    return "host_timeout" if message.start_with?("Host did not answer ")
    return "host_connection_closed" if message.start_with?("Host closed the connection ")
    return "provider_deadline_exceeded" if message.include?("provider request deadline exceeded")
    return "provider_transport_error" if message.include?("provider request failed:")
    "host_request_failed"
  end
end

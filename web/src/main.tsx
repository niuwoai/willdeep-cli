import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { App } from "./App";
import "./theme.css";
import "./styles.css";
import "./routing.css";
import "./attachments.css";
import "./plugins.css";

const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: 1000 }, mutations: { retry: false } } });
createRoot(document.getElementById("root")!).render(
  <StrictMode><QueryClientProvider client={queryClient}><ChakraProvider value={defaultSystem}><App /></ChakraProvider></QueryClientProvider></StrictMode>,
);
